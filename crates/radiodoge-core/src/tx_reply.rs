//! Gateway → sender replies for a radioed transaction, plus the sender-side
//! duplicate-broadcast guard.
//!
//! A gateway answers a `CMD_DOGE_TX` with one LoRa text message:
//!
//! | Text                     | Meaning                                                     |
//! |--------------------------|-------------------------------------------------------------|
//! | `TX_ACK:<txid>`          | on the network (broadcast, or already known)                |
//! | `TX_QUEUED:<txid>`       | gateway could not reach the network; held and retried       |
//! | `TX_NACK:<txid>:<why>`   | network rejected it — resending the same bytes will not help |
//!
//! Older gateways only ever send `TX_ACK`, so a sender must treat silence as
//! "unknown", never as failure.

use std::collections::HashMap;

/// Block explorer used for txid links in the CLI and GUI.
pub const EXPLORER_TX_URL: &str = "https://dogechain.info/tx/";

/// How long the sender refuses to radio the same txid again without `--force`.
pub const RESEND_GUARD_SECS: u64 = 30 * 60;

/// Longest reason kept in a `TX_NACK` so the whole reply stays one LoRa packet.
pub const MAX_NACK_REASON: usize = 100;

/// Explorer link for a txid.
pub fn explorer_url(txid: &str) -> String {
    format!("{}{}", EXPLORER_TX_URL, txid)
}

/// A gateway's answer to a radioed transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TxReply {
    Ack { txid: String },
    Queued { txid: String },
    Nack { txid: String, reason: String },
}

impl TxReply {
    pub fn txid(&self) -> &str {
        match self {
            TxReply::Ack { txid } | TxReply::Queued { txid } | TxReply::Nack { txid, .. } => txid,
        }
    }

    /// Wire text, e.g. `TX_NACK:<txid>:bad-txns-inputs-missingorspent`.
    pub fn to_text(&self) -> String {
        match self {
            TxReply::Ack { txid } => format!("TX_ACK:{}", txid),
            TxReply::Queued { txid } => format!("TX_QUEUED:{}", txid),
            TxReply::Nack { txid, reason } => {
                let r: String = reason.replace(['\r', '\n'], " ").chars().take(MAX_NACK_REASON).collect();
                format!("TX_NACK:{}:{}", txid, r.trim())
            }
        }
    }

    /// Find a reply anywhere in a decoded message (payloads may carry padding).
    pub fn parse(text: &str) -> Option<TxReply> {
        let t = text.trim_end_matches('\0');
        let after = |tag: &str| t.find(tag).map(|i| t[i + tag.len()..].trim());
        if let Some(rest) = after("TX_NACK:") {
            let (id, why) = rest.split_once(':').unwrap_or((rest, ""));
            return Some(TxReply::Nack { txid: id.trim().to_string(), reason: why.trim().to_string() });
        }
        if let Some(rest) = after("TX_QUEUED:") {
            return Some(TxReply::Queued { txid: rest.to_string() });
        }
        after("TX_ACK:").map(|rest| TxReply::Ack { txid: rest.to_string() })
    }
}

/// `true` when a broadcast error is permanent: the network judged the
/// transaction itself, so retrying the same bytes cannot succeed. Anything not
/// recognised is treated as transient (keep it queued) — dropping a valid
/// transaction is worse than retrying a dead one.
pub fn is_permanent_rejection(err: &str) -> bool {
    let e = err.to_ascii_lowercase();
    [
        "bad-txns", "missing inputs", "missingorspent", "insufficient fee", "min relay fee",
        "mandatory-script-verify", "non-mandatory-script-verify", "dust", "txn-mempool-conflict",
        "non-final", "absurdly-high-fee", "invalid transaction", "tx decode failed",
    ]
    .iter()
    .any(|k| e.contains(k))
}

/// Txids this sender has radioed recently (persisted by the caller as JSON).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RecentSends {
    sent: HashMap<String, u64>,
}

impl RecentSends {
    pub fn from_json(s: &str) -> Self {
        RecentSends { sent: serde_json::from_str(s).unwrap_or_default() }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(&self.sent).unwrap_or_else(|_| "{}".into())
    }

    /// Seconds since `txid` was last radioed, if within the guard window.
    pub fn recently_sent(&self, txid: &str, now: u64) -> Option<u64> {
        self.sent.get(txid).map(|&t| now.saturating_sub(t)).filter(|&age| age < RESEND_GUARD_SECS)
    }

    /// Forget a send that never fully reached the air.
    pub fn forget(&mut self, txid: &str) {
        self.sent.remove(txid);
    }

    /// Record a send and drop entries older than a day.
    pub fn record(&mut self, txid: &str, now: u64) {
        self.sent.retain(|_, t| now.saturating_sub(*t) < 86_400);
        self.sent.insert(txid.to_string(), now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const ID: &str = "5b2a3f53f605d62c53e62932dac6925e3d74afa5a4b459745c36d42d0ed26a69";

    #[test]
    fn round_trips_and_fits_one_packet() {
        for r in [
            TxReply::Ack { txid: ID.into() },
            TxReply::Queued { txid: ID.into() },
            TxReply::Nack { txid: ID.into(), reason: "bad-txns-inputs-missingorspent".into() },
        ] {
            assert_eq!(TxReply::parse(&r.to_text()), Some(r.clone()));
        }
        let long = TxReply::Nack { txid: ID.into(), reason: "x".repeat(500) };
        assert!(long.to_text().len() <= 8 + 64 + 1 + MAX_NACK_REASON);
        assert!(long.to_text().len() <= crate::radio::MULTIPART_CHUNK_LEN);
    }

    #[test]
    fn parses_padded_and_legacy() {
        assert_eq!(TxReply::parse(&format!("TX_ACK:{}\0\0", ID)), Some(TxReply::Ack { txid: ID.into() }));
        assert_eq!(TxReply::parse("hello"), None);
        assert_eq!(TxReply::parse("TX_NACK:abc"), Some(TxReply::Nack { txid: "abc".into(), reason: String::new() }));
    }

    #[test]
    fn classifies_rejections() {
        assert!(is_permanent_rejection("RPC error -26: bad-txns-inputs-missingorspent"));
        assert!(is_permanent_rejection("Missing inputs"));
        assert!(!is_permanent_rejection("connection timed out"));
        assert!(!is_permanent_rejection("HTTP 503"));
    }

    #[test]
    fn resend_guard_window() {
        let mut r = RecentSends::default();
        r.record(ID, 1000);
        assert_eq!(r.recently_sent(ID, 1060), Some(60));
        assert_eq!(r.recently_sent(ID, 1000 + RESEND_GUARD_SECS), None);
        let r2 = RecentSends::from_json(&r.to_json());
        assert_eq!(r2, r);
        assert_eq!(RecentSends::from_json("garbage"), RecentSends::default());
        r.record("other", 1000 + 90_000);
        assert_eq!(r.recently_sent(ID, 1000 + 90_000), None);
    }

    #[test]
    fn explorer_link() {
        assert_eq!(explorer_url(ID), format!("https://dogechain.info/tx/{}", ID));
    }
}
