//! QR / payment-URI parsing helpers.
//!
//! QR codes for Dogecoin encode either a bare address (`D…`) or a BIP21-style
//! payment URI (`dogecoin:D…?amount=1.5&label=…`). This module turns either
//! form into a `(address, amount)` pair for the Send flow. The actual image
//! decoding lives in the GUI backend (which owns the `image`/`rqrr` deps); this
//! module is the pure, unit-tested text layer shared by every front end.

/// The result of interpreting scanned/pasted QR text.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedPayment {
    /// The Dogecoin address (validated to look like a mainnet `D…` address).
    pub address: String,
    /// The requested amount in DOGE, if the URI carried an `amount=` parameter.
    pub amount: Option<f64>,
    /// The `label`/`message` parameter, if present (useful as a memo).
    pub label: Option<String>,
}

/// Percent-decode a BIP21 URI component (enough for labels/messages).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                if let (Some(h), Some(l)) = (hi, lo) {
                    out.push((h * 16 + l) as u8);
                    i += 3;
                    continue;
                }
                out.push(b'%');
                i += 1;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A cheap structural check for a Dogecoin mainnet P2PKH address string.
///
/// This mirrors the length/prefix gate the UI uses; full base58check validation
/// is available via [`crate::wallet::is_valid_address`].
fn looks_like_doge_address(s: &str) -> bool {
    let s = s.trim();
    s.starts_with('D')
        && (26..=34).contains(&s.len())
        && s.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Parse scanned QR text (or a pasted string) into a [`ParsedPayment`].
///
/// Accepts:
/// - A bare address: `DH5yaieqoZN36fDVciNyRueRGvGLR3mr7L`
/// - A BIP21 URI: `dogecoin:DH5y…?amount=4.2&label=Coffee`
///
/// The `dogecoin:` scheme match is case-insensitive. Returns `None` if no
/// plausible Dogecoin address can be extracted.
pub fn parse_payment(text: &str) -> Option<ParsedPayment> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }

    // Split off an optional "scheme:" prefix (case-insensitive "dogecoin").
    let without_scheme = match text.split_once(':') {
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("dogecoin") => rest,
        // A bare address has no colon; anything else with an unknown scheme is rejected.
        Some(_) => return None,
        None => text,
    };

    // Separate the address from the query string.
    let (addr_part, query) = match without_scheme.split_once('?') {
        Some((a, q)) => (a, Some(q)),
        None => (without_scheme, None),
    };
    let address = addr_part.trim();
    if !looks_like_doge_address(address) {
        return None;
    }

    let mut amount = None;
    let mut label = None;
    if let Some(q) = query {
        for pair in q.split('&') {
            let (key, val) = match pair.split_once('=') {
                Some(kv) => kv,
                None => continue,
            };
            match key.to_ascii_lowercase().as_str() {
                "amount" => {
                    // BIP21 amounts are decimal DOGE; reject non-positive / unparsable.
                    if let Ok(a) = val.parse::<f64>() {
                        if a.is_finite() && a > 0.0 {
                            amount = Some(a);
                        }
                    }
                }
                "label" | "message" if label.is_none() => {
                    let decoded = percent_decode(val);
                    if !decoded.is_empty() {
                        label = Some(decoded);
                    }
                }
                _ => {}
            }
        }
    }

    Some(ParsedPayment {
        address: address.to_string(),
        amount,
        label,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADDR: &str = "DH5yaieqoZN36fDVciNyRueRGvGLR3mr7L";

    #[test]
    fn test_bare_address() {
        let p = parse_payment(ADDR).unwrap();
        assert_eq!(p.address, ADDR);
        assert_eq!(p.amount, None);
        assert_eq!(p.label, None);
    }

    #[test]
    fn test_bare_address_with_whitespace() {
        let p = parse_payment(&format!("  {ADDR}\n")).unwrap();
        assert_eq!(p.address, ADDR);
    }

    #[test]
    fn test_dogecoin_uri_with_amount() {
        let p = parse_payment(&format!("dogecoin:{ADDR}?amount=4.2")).unwrap();
        assert_eq!(p.address, ADDR);
        assert_eq!(p.amount, Some(4.2));
    }

    #[test]
    fn test_uri_scheme_is_case_insensitive() {
        let p = parse_payment(&format!("DOGECOIN:{ADDR}")).unwrap();
        assert_eq!(p.address, ADDR);
    }

    #[test]
    fn test_uri_with_label_and_amount() {
        let p = parse_payment(&format!("dogecoin:{ADDR}?amount=10&label=Coffee%20Fund")).unwrap();
        assert_eq!(p.amount, Some(10.0));
        assert_eq!(p.label.as_deref(), Some("Coffee Fund"));
    }

    #[test]
    fn test_message_used_as_label() {
        let p = parse_payment(&format!("dogecoin:{ADDR}?message=much+wow")).unwrap();
        assert_eq!(p.label.as_deref(), Some("much wow"));
    }

    #[test]
    fn test_invalid_amount_ignored() {
        let p = parse_payment(&format!("dogecoin:{ADDR}?amount=notanumber")).unwrap();
        assert_eq!(p.amount, None);
        let p = parse_payment(&format!("dogecoin:{ADDR}?amount=-5")).unwrap();
        assert_eq!(p.amount, None);
    }

    #[test]
    fn test_wrong_scheme_rejected() {
        assert!(parse_payment(&format!("bitcoin:{ADDR}")).is_none());
        assert!(parse_payment("https://example.com").is_none());
    }

    #[test]
    fn test_non_address_rejected() {
        assert!(parse_payment("hello world").is_none());
        assert!(parse_payment("").is_none());
        assert!(parse_payment("dogecoin:not-an-address").is_none());
    }
}
