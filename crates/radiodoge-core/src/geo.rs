//! GPS position sharing over the mesh.
//!
//! A node can broadcast where it is, so responders can see where the nodes on a
//! disaster-response mesh are. A position goes out as an ordinary text MESSAGE,
//! which every firmware version already relays:
//!
//! ```text
//! POS:<lat>,<lon>[,<accuracy m>]      e.g.  POS:44.97781,-93.26504,15
//! ```
//!
//! Coordinates use 5 decimal places (about 1.1 m), which keeps the packet to
//! 30-odd bytes. A position is only sent when the user presses the button.
//! Nothing is attached to transactions automatically, because LoRa traffic is
//! not encrypted and anyone in range can read it.

use serde::{Deserialize, Serialize};

/// Text prefix that marks a position message.
pub const POS_PREFIX: &str = "POS:";

/// A WGS-84 position.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Position {
    pub lat: f64,
    pub lon: f64,
    /// Horizontal accuracy in metres, when the source knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accuracy_m: Option<u32>,
}

impl Position {
    /// A position, rejecting out-of-range or non-finite coordinates.
    pub fn new(lat: f64, lon: f64, accuracy_m: Option<u32>) -> Option<Self> {
        let ok = lat.is_finite()
            && lon.is_finite()
            && (-90.0..=90.0).contains(&lat)
            && (-180.0..=180.0).contains(&lon);
        ok.then_some(Self {
            lat,
            lon,
            accuracy_m,
        })
    }

    /// The wire text, `POS:lat,lon[,acc]`.
    pub fn to_message(&self) -> String {
        let mut s = format!("{}{:.5},{:.5}", POS_PREFIX, self.lat, self.lon);
        if let Some(a) = self.accuracy_m {
            s.push_str(&format!(",{}", a));
        }
        s
    }

    /// Parse message text. Returns `None` for anything that is not a valid position.
    pub fn from_message(text: &str) -> Option<Self> {
        let body = text
            .trim_end_matches('\0')
            .trim()
            .strip_prefix(POS_PREFIX)?;
        let mut parts = body.split(',').map(str::trim);
        let lat = parts.next()?.parse::<f64>().ok()?;
        let lon = parts.next()?.parse::<f64>().ok()?;
        let acc = match parts.next() {
            Some(a) => Some(a.parse::<u32>().ok()?),
            None => None,
        };
        if parts.next().is_some() {
            return None;
        }
        Self::new(lat, lon, acc)
    }

    /// The same text the packet log shows, e.g. `📍 Position: 44.97781, -93.26504 (±15 m)`.
    pub fn describe(&self) -> String {
        let acc = self
            .accuracy_m
            .map(|a| format!(" (±{} m)", a))
            .unwrap_or_default();
        format!("📍 Position: {:.5}, {:.5}{}", self.lat, self.lon, acc)
    }

    /// An OpenStreetMap link centred on the position.
    pub fn map_url(&self) -> String {
        format!(
            "https://www.openstreetmap.org/?mlat={:.5}&mlon={:.5}#map=16/{:.5}/{:.5}",
            self.lat, self.lon, self.lat, self.lon
        )
    }

    /// Great-circle distance to `other` in metres (haversine).
    pub fn distance_m(&self, other: &Position) -> f64 {
        const R: f64 = 6_371_000.0;
        let (p1, p2) = (self.lat.to_radians(), other.lat.to_radians());
        let dp = p2 - p1;
        let dl = (other.lon - self.lon).to_radians();
        let a = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
        2.0 * R * a.sqrt().asin()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let p = Position::new(44.977_812, -93.265_04, Some(15)).unwrap();
        let m = p.to_message();
        assert_eq!(m, "POS:44.97781,-93.26504,15");
        let q = Position::from_message(&m).unwrap();
        assert!((q.lat - 44.97781).abs() < 1e-9 && (q.lon + 93.26504).abs() < 1e-9);
        assert_eq!(q.accuracy_m, Some(15));
        assert_eq!(
            Position::from_message("POS:1.5,2.5").unwrap().accuracy_m,
            None
        );
        assert!(m.len() < 40);
    }

    #[test]
    fn rejects_garbage() {
        for bad in [
            "",
            "POS:",
            "POS:1",
            "POS:91,0",
            "POS:0,181",
            "POS:NaN,0",
            "POS:1,2,x",
            "POS:1,2,3,4",
            "hello",
            "POS:inf,0",
        ] {
            assert!(Position::from_message(bad).is_none(), "{bad}");
        }
        assert!(
            Position::from_message("POS: 1.0 , 2.0 \0\0").is_some(),
            "padding and spaces tolerated"
        );
    }

    #[test]
    fn describe_and_link() {
        let p = Position::new(-33.86882, 151.20929, None).unwrap();
        assert_eq!(p.describe(), "📍 Position: -33.86882, 151.20929");
        assert!(p.map_url().contains("mlat=-33.86882&mlon=151.20929"));
    }

    #[test]
    fn distance() {
        let a = Position::new(0.0, 0.0, None).unwrap();
        let b = Position::new(0.0, 1.0, None).unwrap();
        assert!((a.distance_m(&b) - 111_195.0).abs() < 50.0);
        assert!(a.distance_m(&a) < 1e-6);
    }
}
