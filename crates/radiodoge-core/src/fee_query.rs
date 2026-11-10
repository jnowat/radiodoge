//! Gateway fee-rate query over LoRa (request/response, no beacons).
//!
//! An offline sender has no way to see the Dogecoin mempool, so it asks a
//! gateway that does. The exchange reuses the plain `CMD_MESSAGE` text path the
//! balance reply already rides on, so no firmware change is needed: every board
//! forwards a MESSAGE addressed to it to its USB host.
//!
//! | Direction          | Text                 | Meaning                                    |
//! |--------------------|----------------------|--------------------------------------------|
//! | sender -> gateway  | `FEE?`               | "what fee rate should I pay?"              |
//! | gateway -> sender  | `FEE:<koinus_per_kb>` | current rate, koinus per started kB       |
//!
//! A gateway that cannot reach a chain backend stays silent rather than
//! guessing, so silence always means "fall back to the offline default".
//!
//! **A reported rate is not verified.** Anyone in radio range can send
//! `FEE:<n>`. The client therefore applies its own floor (the recommended
//! 0.01 DOGE/kB) and cap ([`MAX_GATEWAY_FEE_RATE_KOINUS_PER_KB`]) and treats an
//! out-of-range rate as no answer at all. See [`apply_local_bounds`].
//!
//! Everything here is pure data with no I/O (and no `native` feature), so the
//! WebAssembly inspector can decode the messages too.

use std::collections::HashMap;

/// Text a sender radios to ask a gateway for its fee rate.
pub const FEE_QUERY_TEXT: &str = "FEE?";
/// Prefix of a gateway's fee-rate reply; the rest is koinus per started kB.
pub const FEE_REPLY_PREFIX: &str = "FEE:";

/// Local floor: Dogecoin Core's recommended 0.01 DOGE per kB, in koinus.
/// Mirrors `wallet::MIN_FEE_RATE_KOINUS_PER_KB` (checked by a test).
pub const MIN_GATEWAY_FEE_RATE_KOINUS_PER_KB: u64 = 1_000_000;

/// Local cap: a rate that would make a 1 kB transaction cost more than
/// `wallet::MAX_AUTO_FEE_DOGE` (10 DOGE) is refused, not paid.
pub const MAX_GATEWAY_FEE_RATE_KOINUS_PER_KB: u64 = 1_000_000_000;

/// Fee rate used when no acceptable gateway answer arrives: the same 0.01 DOGE
/// per kB the offline Send UI already suggests.
pub const FALLBACK_FEE_RATE_KOINUS_PER_KB: u64 = MIN_GATEWAY_FEE_RATE_KOINUS_PER_KB;

/// How long a sender waits for a reply by default.
pub const DEFAULT_FEE_QUERY_TIMEOUT_SECS: u64 = 20;

/// How long a gateway reuses a fetched rate before asking its backends again.
pub const GATEWAY_FEE_CACHE_SECS: u64 = 120;

/// Minimum gap between two replies to the same node, so one chatty (or hostile)
/// node cannot keep the gateway transmitting.
pub const GATEWAY_FEE_REPLY_INTERVAL_SECS: u64 = 10;

/// Most nodes the reply limiter remembers before pruning old entries.
const MAX_LIMITER_NODES: usize = 256;

/// `true` if `text` (a decoded MESSAGE payload) is a fee-rate query.
pub fn is_fee_query(text: &str) -> bool {
    text.trim_matches('\0').trim() == FEE_QUERY_TEXT
}

/// The gateway's reply text for `rate_koinus_per_kb`.
pub fn format_fee_reply(rate_koinus_per_kb: u64) -> String {
    format!("{}{}", FEE_REPLY_PREFIX, rate_koinus_per_kb)
}

/// Parse a gateway's `FEE:<koinus_per_kb>` reply. Trailing NULs/whitespace are
/// ignored; anything else after the digits makes it unparseable.
pub fn parse_fee_reply(text: &str) -> Option<u64> {
    let t = text.trim_matches('\0').trim();
    let digits = t.strip_prefix(FEE_REPLY_PREFIX)?.trim();
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// Find a fee reply anywhere in a raw MESSAGE payload (some firmware builds
/// prepend display text), the way the CLI finds `BAL:`.
pub fn find_fee_reply(raw: &str) -> Option<u64> {
    let i = raw.find(FEE_REPLY_PREFIX)?;
    let tail: String = raw[i + FEE_REPLY_PREFIX.len()..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if tail.is_empty() {
        return None;
    }
    tail.parse().ok()
}

/// Where the fee rate a client ends up using came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FeeRateSource {
    /// The gateway's rate, inside the local bounds, used as reported.
    Gateway,
    /// The gateway reported less than the local floor; the floor is used.
    GatewayFloored,
    /// No usable answer (timeout, unparseable, or above the cap): the offline default.
    Fallback,
}

/// The fee rate a client should use after hearing (or not hearing) a gateway.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FeeRateChoice {
    /// Rate to sign with, koinus per started kB (always inside the local bounds).
    pub rate_koinus_per_kb: u64,
    /// What the gateway actually said, if anything.
    pub reported_koinus_per_kb: Option<u64>,
    pub source: FeeRateSource,
    /// One-line human explanation for the CLI/GUI.
    pub note: String,
}

/// Apply the client's floor and cap to a gateway reply (`None` = no reply).
///
/// - below the floor: raised to the floor (`GatewayFloored`);
/// - above the cap: refused, offline default used (`Fallback`) — a gateway
///   must never be able to make the user overpay;
/// - no reply: offline default (`Fallback`).
pub fn apply_local_bounds(reported: Option<u64>) -> FeeRateChoice {
    match reported {
        None => FeeRateChoice {
            rate_koinus_per_kb: FALLBACK_FEE_RATE_KOINUS_PER_KB,
            reported_koinus_per_kb: None,
            source: FeeRateSource::Fallback,
            note: "No gateway fee reply - using the offline default (0.01 DOGE per kB).".into(),
        },
        Some(r) if r > MAX_GATEWAY_FEE_RATE_KOINUS_PER_KB => FeeRateChoice {
            rate_koinus_per_kb: FALLBACK_FEE_RATE_KOINUS_PER_KB,
            reported_koinus_per_kb: Some(r),
            source: FeeRateSource::Fallback,
            note: format!(
                "Gateway reported {} DOGE/kB, above the {} DOGE/kB cap - ignored; using the offline default (0.01 DOGE per kB).",
                koinus_to_doge_str(r),
                koinus_to_doge_str(MAX_GATEWAY_FEE_RATE_KOINUS_PER_KB)
            ),
        },
        Some(r) if r < MIN_GATEWAY_FEE_RATE_KOINUS_PER_KB => FeeRateChoice {
            rate_koinus_per_kb: MIN_GATEWAY_FEE_RATE_KOINUS_PER_KB,
            reported_koinus_per_kb: Some(r),
            source: FeeRateSource::GatewayFloored,
            note: format!(
                "Gateway reported {} DOGE/kB, below the recommended floor - using 0.01 DOGE per kB (unverified).",
                koinus_to_doge_str(r)
            ),
        },
        Some(r) => FeeRateChoice {
            rate_koinus_per_kb: r,
            reported_koinus_per_kb: Some(r),
            source: FeeRateSource::Gateway,
            note: format!(
                "Gateway reports {} DOGE/kB (from gateway, not verified).",
                koinus_to_doge_str(r)
            ),
        },
    }
}

/// Fee in koinus at `rate` for a transaction of `size_bytes`, charged per
/// started kB (same rounding as `wallet::fee_for_size`, floor applied).
pub fn fee_koinus_for_size(rate_koinus_per_kb: u64, size_bytes: usize) -> u64 {
    let rate = rate_koinus_per_kb.max(MIN_GATEWAY_FEE_RATE_KOINUS_PER_KB);
    let kb = size_bytes.div_ceil(1000).max(1) as u64;
    kb.saturating_mul(rate)
}

fn koinus_to_doge_str(k: u64) -> String {
    let s = format!("{}.{:08}", k / 100_000_000, k % 100_000_000);
    let s = s.trim_end_matches('0');
    s.strip_suffix('.').unwrap_or(s).to_string()
}

/// Gateway side: remembers the last fetched rate for [`GATEWAY_FEE_CACHE_SECS`]
/// so a burst of queries does not hammer the chain backends.
#[derive(Debug, Default, Clone)]
pub struct FeeRateCache {
    entry: Option<(u64, u64)>, // (rate, fetched_at)
}

impl FeeRateCache {
    pub fn new() -> Self {
        Self::default()
    }
    /// The cached rate if it is still fresh at `now_secs`.
    pub fn get(&self, now_secs: u64) -> Option<u64> {
        self.entry
            .filter(|(_, at)| now_secs.saturating_sub(*at) < GATEWAY_FEE_CACHE_SECS)
            .map(|(r, _)| r)
    }
    pub fn put(&mut self, rate_koinus_per_kb: u64, now_secs: u64) {
        self.entry = Some((rate_koinus_per_kb, now_secs));
    }
}

/// Gateway side: at most one fee reply per node per
/// [`GATEWAY_FEE_REPLY_INTERVAL_SECS`].
#[derive(Debug, Default, Clone)]
pub struct FeeReplyLimiter {
    last: HashMap<String, u64>,
}

impl FeeReplyLimiter {
    pub fn new() -> Self {
        Self::default()
    }
    /// `true` (and records the reply) if `node` may be answered at `now_secs`.
    pub fn allow(&mut self, node: &str, now_secs: u64) -> bool {
        if let Some(&t) = self.last.get(node) {
            if now_secs.saturating_sub(t) < GATEWAY_FEE_REPLY_INTERVAL_SECS {
                return false;
            }
        }
        if self.last.len() >= MAX_LIMITER_NODES {
            self.last
                .retain(|_, t| now_secs.saturating_sub(*t) < GATEWAY_FEE_REPLY_INTERVAL_SECS);
        }
        self.last.insert(node.to_string(), now_secs);
        true
    }
}

/// What a gateway should do with an incoming MESSAGE.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatewayFeeAction {
    /// Not a fee query - ignore.
    NotAQuery,
    /// A query from a node answered too recently - drop it.
    RateLimited,
    /// Answer with this text (cached rate).
    Reply(String),
    /// Answer after fetching a fresh rate from the backends.
    FetchThenReply,
}

/// Decide how a gateway handles `text` from `node` at `now_secs`. A cached rate
/// is answered immediately; otherwise the caller fetches, calls
/// [`FeeRateCache::put`], and replies with [`format_fee_reply`].
pub fn gateway_fee_action(
    text: &str,
    node: &str,
    now_secs: u64,
    cache: &FeeRateCache,
    limiter: &mut FeeReplyLimiter,
) -> GatewayFeeAction {
    if !is_fee_query(text) {
        return GatewayFeeAction::NotAQuery;
    }
    if !limiter.allow(node, now_secs) {
        return GatewayFeeAction::RateLimited;
    }
    match cache.get(now_secs) {
        Some(r) => GatewayFeeAction::Reply(format_fee_reply(r)),
        None => GatewayFeeAction::FetchThenReply,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_and_reply_round_trip() {
        assert!(is_fee_query("FEE?"));
        assert!(is_fee_query(" FEE?\0\0"));
        assert!(!is_fee_query("FEE"));
        assert!(!is_fee_query("FEE:100"));
        assert_eq!(format_fee_reply(1_500_000), "FEE:1500000");
        assert_eq!(parse_fee_reply("FEE:1500000"), Some(1_500_000));
        assert_eq!(parse_fee_reply("FEE:1500000\0"), Some(1_500_000));
        assert_eq!(parse_fee_reply("FEE:"), None);
        assert_eq!(parse_fee_reply("FEE:-5"), None);
        assert_eq!(parse_fee_reply("FEE:12abc"), None);
        assert_eq!(parse_fee_reply("BAL:12"), None);
        assert_eq!(parse_fee_reply("FEE:99999999999999999999999"), None);
    }

    #[test]
    fn find_fee_reply_inside_noise() {
        assert_eq!(find_fee_reply("[gw] FEE:2000000\0\0"), Some(2_000_000));
        assert_eq!(find_fee_reply("FEE?"), None);
        assert_eq!(find_fee_reply("nothing"), None);
    }

    #[test]
    fn bounds_floor_cap_and_fallback() {
        let c = apply_local_bounds(None);
        assert_eq!(c.source, FeeRateSource::Fallback);
        assert_eq!(c.rate_koinus_per_kb, FALLBACK_FEE_RATE_KOINUS_PER_KB);

        let c = apply_local_bounds(Some(10));
        assert_eq!(c.source, FeeRateSource::GatewayFloored);
        assert_eq!(c.rate_koinus_per_kb, MIN_GATEWAY_FEE_RATE_KOINUS_PER_KB);
        assert_eq!(c.reported_koinus_per_kb, Some(10));

        let c = apply_local_bounds(Some(2_500_000));
        assert_eq!(c.source, FeeRateSource::Gateway);
        assert_eq!(c.rate_koinus_per_kb, 2_500_000);
        assert!(c.note.contains("0.025"));

        let c = apply_local_bounds(Some(MAX_GATEWAY_FEE_RATE_KOINUS_PER_KB));
        assert_eq!(c.source, FeeRateSource::Gateway);

        let c = apply_local_bounds(Some(MAX_GATEWAY_FEE_RATE_KOINUS_PER_KB + 1));
        assert_eq!(c.source, FeeRateSource::Fallback);
        assert_eq!(c.rate_koinus_per_kb, FALLBACK_FEE_RATE_KOINUS_PER_KB);
        assert!(c.note.contains("cap"));
    }

    #[test]
    fn fee_for_size_per_started_kb() {
        assert_eq!(fee_koinus_for_size(2_000_000, 226), 2_000_000);
        assert_eq!(fee_koinus_for_size(2_000_000, 1001), 4_000_000);
        assert_eq!(fee_koinus_for_size(0, 0), 1_000_000);
    }

    #[cfg(feature = "native")]
    #[test]
    fn bounds_match_wallet_constants() {
        use crate::wallet;
        assert_eq!(
            MIN_GATEWAY_FEE_RATE_KOINUS_PER_KB,
            wallet::MIN_FEE_RATE_KOINUS_PER_KB
        );
        assert_eq!(
            MAX_GATEWAY_FEE_RATE_KOINUS_PER_KB as f64 / 1e8,
            wallet::MAX_AUTO_FEE_DOGE
        );
        for size in [0usize, 226, 999, 1000, 1001, 3700] {
            assert_eq!(
                fee_koinus_for_size(3_000_000, size),
                wallet::fee_for_size(3_000_000, size)
            );
        }
    }

    #[test]
    fn cache_expires() {
        let mut c = FeeRateCache::new();
        assert_eq!(c.get(100), None);
        c.put(1_234_567, 100);
        assert_eq!(c.get(100 + GATEWAY_FEE_CACHE_SECS - 1), Some(1_234_567));
        assert_eq!(c.get(100 + GATEWAY_FEE_CACHE_SECS), None);
    }

    #[test]
    fn limiter_one_reply_per_interval_per_node() {
        let mut l = FeeReplyLimiter::new();
        assert!(l.allow("10.0.2", 1000));
        assert!(!l.allow("10.0.2", 1000 + GATEWAY_FEE_REPLY_INTERVAL_SECS - 1));
        assert!(l.allow("10.0.3", 1001));
        assert!(l.allow("10.0.2", 1000 + GATEWAY_FEE_REPLY_INTERVAL_SECS));
    }

    #[test]
    fn limiter_prunes_when_full() {
        let mut l = FeeReplyLimiter::new();
        for i in 0..MAX_LIMITER_NODES {
            assert!(l.allow(&format!("n{}", i), 0));
        }
        assert!(l.allow("late", GATEWAY_FEE_REPLY_INTERVAL_SECS + 5));
        assert!(l.last.len() <= 2);
    }

    #[test]
    fn simulated_gateway_exchange() {
        // Sender asks, gateway (no cache) fetches, caches, replies; second
        // sender is answered from the cache; first sender re-asking too soon is
        // dropped; client bounds the result.
        let mut cache = FeeRateCache::new();
        let mut lim = FeeReplyLimiter::new();
        assert_eq!(
            gateway_fee_action("hello", "10.0.2", 0, &cache, &mut lim),
            GatewayFeeAction::NotAQuery
        );
        assert_eq!(
            gateway_fee_action(FEE_QUERY_TEXT, "10.0.2", 0, &cache, &mut lim),
            GatewayFeeAction::FetchThenReply
        );
        cache.put(1_800_000, 0);
        let reply = match gateway_fee_action(FEE_QUERY_TEXT, "10.0.3", 1, &cache, &mut lim) {
            GatewayFeeAction::Reply(t) => t,
            other => panic!("expected reply, got {:?}", other),
        };
        assert_eq!(reply, "FEE:1800000");
        assert_eq!(
            gateway_fee_action(FEE_QUERY_TEXT, "10.0.2", 2, &cache, &mut lim),
            GatewayFeeAction::RateLimited
        );
        let choice = apply_local_bounds(parse_fee_reply(&reply));
        assert_eq!(choice.source, FeeRateSource::Gateway);
        assert_eq!(choice.rate_koinus_per_kb, 1_800_000);
    }

    #[test]
    fn radio_decodes_fee_messages() {
        let d = crate::radio::decode_payload(crate::radio::CMD_MESSAGE, b"FEE:2000000\0").unwrap();
        assert!(
            d.contains("0.02000000 DOGE/kB") && d.contains("unverified"),
            "{}",
            d
        );
        let q = crate::radio::decode_payload(crate::radio::CMD_MESSAGE, b"FEE?").unwrap();
        assert!(q.contains("Fee rate request"), "{}", q);
    }

    #[test]
    fn doge_formatting() {
        assert_eq!(koinus_to_doge_str(1_000_000), "0.01");
        assert_eq!(koinus_to_doge_str(1_000_000_000), "10");
        assert_eq!(koinus_to_doge_str(0), "0");
    }
}
