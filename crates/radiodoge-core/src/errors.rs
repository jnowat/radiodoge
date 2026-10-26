//! Friendly, actionable error text for the GUI and CLI.
//!
//! Low-level errors (serial, HTTP, crypto, filesystem) are accurate but rarely
//! tell a user what to *do*. [`hint_for`] recognises the common ones and
//! returns a short next step; [`friendly`] appends it to the original message,
//! which is always kept so nothing diagnostic is lost.

use std::fmt::Display;

/// (needles, hint). Matching is case-insensitive; the first hit wins.
const HINTS: &[(&[&str], &str)] = &[
    (
        &["no serial port open", "connect first", "not connected to a board"],
        "Connect a board first (Connect tab in the app, or --port on the CLI).",
    ),
    (
        &["access is denied", "permission denied", "resource busy", "port is busy"],
        "Another program (Arduino IDE, PuTTY, a second RadioDoge window) may have the port open. Close it and try again.",
    ),
    (
        &["device is not connected", "device has been removed", "broken pipe", "device not configured"],
        "The board went away. Check the USB cable (some USB-C cables are power-only) and reconnect.",
    ),
    (
        &["no space left", "os error 28", "disk is full"],
        "The disk is full. Free up some space and try again.",
    ),
    (
        &["os error 2)", "no such file", "cannot find the path", "cannot find the file"],
        "Check that the file or folder path exists and is spelled correctly.",
    ),
    (
        &["429", "too many requests", "rate limit"],
        "The block explorer is rate-limiting requests. Wait a minute and retry.",
    ),
    (
        &["connection refused", "actively refused"],
        "The chain backend refused the connection. If you use a local Dogecoin Core node, make sure it is running and the RPC settings are right.",
    ),
    (
        &["error sending request", "dns error", "failed to lookup address", "network is unreachable", "no such host"],
        "Couldn't reach the network. Check your internet connection and try again.",
    ),
    (
        &["timed out", "timeout"],
        "No answer in time. Check the board is powered and running RadioDoge firmware (or that you're online), then retry.",
    ),
    (
        &["insufficient funds", "insufficient balance", "not enough funds"],
        "The balance can't cover the amount plus the network fee. Lower the amount or add funds.",
    ),
    (
        &["decryption failed", "aead", "wrong passphrase", "bad passphrase"],
        "The passphrase looks wrong (or the wallet file is damaged). Check it and try again.",
    ),
    (
        &["mnemonic", "invalid word", "word count"],
        "Check the recovery phrase: 12 or 24 words, spelled exactly, separated by single spaces.",
    ),
    (
        &["invalid wif", "invalid checksum", "invalid address", "base58"],
        "Check the key or address for typos. Dogecoin addresses start with D, A or 9.",
    ),
];

/// A short next step for a recognised error, if any.
pub fn hint_for(raw: &str) -> Option<&'static str> {
    let lower = raw.to_ascii_lowercase();
    HINTS
        .iter()
        .find(|(needles, _)| needles.iter().any(|n| lower.contains(n)))
        .map(|(_, hint)| *hint)
}

/// The error's own text, followed by `Hint: …` when one applies.
///
/// Messages that already carry a hint are returned unchanged.
pub fn friendly(err: impl Display) -> String {
    let raw = err.to_string();
    if raw.contains("Hint:") {
        return raw;
    }
    match hint_for(&raw) {
        Some(hint) => format!("{raw}\nHint: {hint}"),
        None => raw,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_errors_get_a_hint() {
        let s = friendly("No serial port open — connect first");
        assert!(s.starts_with("No serial port open"));
        assert!(s.contains("Hint: Connect a board first"));
        assert!(hint_for("Access is denied. (os error 5)").unwrap().contains("Arduino IDE"));
        assert!(hint_for("operation timed out").unwrap().contains("No answer"));
        assert!(hint_for("HTTP 429 Too Many Requests").unwrap().contains("rate-limiting"));
        assert!(hint_for("The system cannot find the file specified. (os error 2)").is_some());
    }

    #[test]
    fn unknown_errors_pass_through() {
        assert_eq!(friendly("something odd"), "something odd");
        assert_eq!(hint_for(""), None);
    }

    #[test]
    fn existing_hints_are_not_doubled() {
        let raw = "Access denied on COM3.\nHint: close the other program.";
        assert_eq!(friendly(raw), raw);
    }

    #[test]
    fn matching_is_case_insensitive() {
        assert!(hint_for("CONNECTION REFUSED").is_some());
    }
}
