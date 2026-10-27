//! Meshtastic over-the-air packet encoding (ROADMAP v0.5.x: Meshtastic interop).
//!
//! Builds and parses the frame a Meshtastic node puts on the air:
//!
//! ```text
//! [0..4)   to        u32 LE   (0xFFFF_FFFF = broadcast)
//! [4..8)   from      u32 LE
//! [8..12)  id        u32 LE
//! [12]     flags     hop_limit bits 0-2, want_ack bit 3, via_mqtt bit 4, hop_start bits 5-7
//! [13]     channel   channel hash (XOR of name bytes ^ XOR of key bytes)
//! [14]     next_hop
//! [15]     relay_node
//! [16..]   AES-CTR(key, nonce = id u64 LE ‖ from u32 LE ‖ 0u32) of a `Data` protobuf
//! ```
//!
//! RadioDoge payloads are wrapped in `Data { portnum: PRIVATE_APP (256), payload }`
//! so standard Meshtastic nodes relay them without trying to interpret them.
//! The protobuf subset is hand-rolled (fields 1 portnum and 2 payload; unknown
//! fields are skipped) to avoid pulling in a protobuf toolchain.
//!
//! Note: actually transmitting these frames needs a firmware raw-TX command and
//! a radio tuned to the Meshtastic preset (see `scan::plan`, "Meshtastic LongFast").

use aes::cipher::{KeyIvInit, StreamCipher};

type Aes128Ctr = ctr::Ctr128BE<aes::Aes128>;
type Aes256Ctr = ctr::Ctr128BE<aes::Aes256>;

pub const HEADER_LEN: usize = 16;
pub const BROADCAST: u32 = 0xFFFF_FFFF;
/// Max LoRa frame (255) minus header.
pub const MAX_ENCRYPTED_LEN: usize = 255 - HEADER_LEN;

pub const PORT_TEXT_MESSAGE: u32 = 1;
pub const PORT_PRIVATE_APP: u32 = 256;

/// Expanded default channel key (PSK index 1, "AQ==" in the Meshtastic apps).
pub const DEFAULT_KEY: [u8; 16] = [
    0xd4, 0xf1, 0xbb, 0x3a, 0x20, 0x29, 0x07, 0x59, 0xf0, 0xbc, 0xff, 0xab, 0xcf, 0x4e, 0x69, 0x01,
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeshError {
    TooShort,
    TooLong(usize),
    BadKey(usize),
    BadProtobuf(&'static str),
}

impl std::fmt::Display for MeshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooShort => write!(f, "frame shorter than the 16-byte Meshtastic header"),
            Self::TooLong(n) => write!(
                f,
                "encrypted payload is {n} bytes; max is {MAX_ENCRYPTED_LEN}"
            ),
            Self::BadKey(n) => write!(f, "channel key must be 0, 1, 16 or 32 bytes (got {n})"),
            Self::BadProtobuf(w) => write!(f, "bad Data protobuf: {w}"),
        }
    }
}
impl std::error::Error for MeshError {}

/// A channel = name + key. `key` follows Meshtastic PSK rules: empty means no
/// encryption, a single byte `n` (1..=10) means the default key with its last
/// byte bumped by `n - 1`, and 16/32 bytes is AES-128/256.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Channel {
    pub name: String,
    pub key: Vec<u8>,
}

impl Channel {
    /// The stock public channel ("LongFast", default key).
    pub fn default_longfast() -> Self {
        Self {
            name: "LongFast".into(),
            key: vec![1],
        }
    }

    pub fn expanded_key(&self) -> Result<Vec<u8>, MeshError> {
        match self.key.len() {
            0 => Ok(vec![]),
            1 => {
                let n = self.key[0];
                if n == 0 {
                    return Ok(vec![]);
                }
                let mut k = DEFAULT_KEY.to_vec();
                k[15] = k[15].wrapping_add(n - 1);
                Ok(k)
            }
            16 | 32 => Ok(self.key.clone()),
            n => Err(MeshError::BadKey(n)),
        }
    }

    pub fn hash(&self) -> Result<u8, MeshError> {
        let k = self.expanded_key()?;
        let x = |b: &[u8]| b.iter().fold(0u8, |a, &v| a ^ v);
        Ok(x(self.name.as_bytes()) ^ x(&k))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub to: u32,
    pub from: u32,
    pub id: u32,
    pub hop_limit: u8,
    pub hop_start: u8,
    pub want_ack: bool,
    pub via_mqtt: bool,
    pub channel_hash: u8,
    pub next_hop: u8,
    pub relay_node: u8,
}

impl Header {
    pub fn to_bytes(&self) -> [u8; HEADER_LEN] {
        let mut b = [0u8; HEADER_LEN];
        b[0..4].copy_from_slice(&self.to.to_le_bytes());
        b[4..8].copy_from_slice(&self.from.to_le_bytes());
        b[8..12].copy_from_slice(&self.id.to_le_bytes());
        b[12] = (self.hop_limit & 7)
            | (u8::from(self.want_ack) << 3)
            | (u8::from(self.via_mqtt) << 4)
            | ((self.hop_start & 7) << 5);
        b[13] = self.channel_hash;
        b[14] = self.next_hop;
        b[15] = self.relay_node;
        b
    }

    pub fn parse(b: &[u8]) -> Result<Self, MeshError> {
        if b.len() < HEADER_LEN {
            return Err(MeshError::TooShort);
        }
        let u = |i: usize| u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        Ok(Self {
            to: u(0),
            from: u(4),
            id: u(8),
            hop_limit: b[12] & 7,
            want_ack: b[12] & 0x08 != 0,
            via_mqtt: b[12] & 0x10 != 0,
            hop_start: b[12] >> 5,
            channel_hash: b[13],
            next_hop: b[14],
            relay_node: b[15],
        })
    }
}

/// The decoded `meshtastic.Data` fields we care about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Data {
    pub portnum: u32,
    pub payload: Vec<u8>,
}

fn put_varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

fn get_varint(b: &[u8], i: &mut usize) -> Result<u64, MeshError> {
    let mut v = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *b
            .get(*i)
            .ok_or(MeshError::BadProtobuf("truncated varint"))?;
        *i += 1;
        v |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(v);
        }
    }
    Err(MeshError::BadProtobuf("varint too long"))
}

impl Data {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.payload.len() + 6);
        out.push(0x08); // field 1, varint
        put_varint(&mut out, u64::from(self.portnum));
        out.push(0x12); // field 2, length-delimited
        put_varint(&mut out, self.payload.len() as u64);
        out.extend_from_slice(&self.payload);
        out
    }

    pub fn decode(b: &[u8]) -> Result<Self, MeshError> {
        let (mut i, mut portnum, mut payload) = (0usize, 0u32, Vec::new());
        while i < b.len() {
            let tag = get_varint(b, &mut i)?;
            let (field, wire) = (tag >> 3, tag & 7);
            match wire {
                0 => {
                    let v = get_varint(b, &mut i)?;
                    if field == 1 {
                        portnum =
                            u32::try_from(v).map_err(|_| MeshError::BadProtobuf("portnum"))?;
                    }
                }
                2 => {
                    let len = usize::try_from(get_varint(b, &mut i)?)
                        .map_err(|_| MeshError::BadProtobuf("length"))?;
                    let end = i
                        .checked_add(len)
                        .filter(|&e| e <= b.len())
                        .ok_or(MeshError::BadProtobuf("length past end"))?;
                    if field == 2 {
                        payload = b[i..end].to_vec();
                    }
                    i = end;
                }
                1 => {
                    i += 8;
                }
                5 => {
                    i += 4;
                }
                _ => return Err(MeshError::BadProtobuf("unsupported wire type")),
            }
            if i > b.len() {
                return Err(MeshError::BadProtobuf("field past end"));
            }
        }
        Ok(Self { portnum, payload })
    }
}

fn crypt(key: &[u8], id: u32, from: u32, buf: &mut [u8]) {
    let mut nonce = [0u8; 16];
    nonce[0..8].copy_from_slice(&u64::from(id).to_le_bytes());
    nonce[8..12].copy_from_slice(&from.to_le_bytes());
    match key.len() {
        16 => Aes128Ctr::new(key.into(), &nonce.into()).apply_keystream(buf),
        32 => Aes256Ctr::new(key.into(), &nonce.into()).apply_keystream(buf),
        _ => {} // no encryption
    }
}

/// Build a complete on-air frame. `header.channel_hash` is overwritten from `channel`.
pub fn encode_frame(
    mut header: Header,
    data: &Data,
    channel: &Channel,
) -> Result<Vec<u8>, MeshError> {
    let key = channel.expanded_key()?;
    header.channel_hash = channel.hash()?;
    let mut body = data.encode();
    if body.len() > MAX_ENCRYPTED_LEN {
        return Err(MeshError::TooLong(body.len()));
    }
    crypt(&key, header.id, header.from, &mut body);
    let mut out = header.to_bytes().to_vec();
    out.extend_from_slice(&body);
    Ok(out)
}

/// Parse a frame received on `channel`. Returns `Ok(None)` when the channel hash
/// doesn't match (i.e. the packet belongs to a different channel).
pub fn decode_frame(frame: &[u8], channel: &Channel) -> Result<Option<(Header, Data)>, MeshError> {
    let header = Header::parse(frame)?;
    if header.channel_hash != channel.hash()? {
        return Ok(None);
    }
    let mut body = frame[HEADER_LEN..].to_vec();
    crypt(&channel.expanded_key()?, header.id, header.from, &mut body);
    Ok(Some((header, Data::decode(&body)?)))
}

/// Wrap a raw RadioDoge packet for relay across a Meshtastic mesh.
pub fn wrap_radiodoge(
    from: u32,
    id: u32,
    radiodoge_packet: &[u8],
    channel: &Channel,
) -> Result<Vec<u8>, MeshError> {
    let header = Header {
        to: BROADCAST,
        from,
        id,
        hop_limit: 3,
        hop_start: 3,
        want_ack: false,
        via_mqtt: false,
        channel_hash: 0,
        next_hop: 0,
        relay_node: 0,
    };
    encode_frame(
        header,
        &Data {
            portnum: PORT_PRIVATE_APP,
            payload: radiodoge_packet.to_vec(),
        },
        channel,
    )
}

/// Inverse of [`wrap_radiodoge`]: `Some(packet)` only for PRIVATE_APP frames on `channel`.
pub fn unwrap_radiodoge(frame: &[u8], channel: &Channel) -> Result<Option<Vec<u8>>, MeshError> {
    Ok(decode_frame(frame, channel)?
        .filter(|(_, d)| d.portnum == PORT_PRIVATE_APP)
        .map(|(_, d)| d.payload))
}

// ─── Bridge mode: Meshtastic over the board's raw LoRa commands (FW12) ──────

/// What the bridge made of one frame the board heard in raw RX mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BridgeEvent {
    /// A RadioDoge packet relayed through Meshtastic (PRIVATE_APP).
    RadioDoge { from: u32, packet: Vec<u8> },
    /// A plain Meshtastic text message on the channel.
    Text { from: u32, text: String },
    /// A frame on the channel with another portnum.
    Other { from: u32, portnum: u32 },
    /// Not decodable on this channel (other channel, noise, or truncated).
    Undecodable,
}

/// Build the board command that sends `radiodoge_packet` over Meshtastic.
pub fn bridge_tx_command(
    host: &crate::types::NodeAddress,
    from: u32,
    id: u32,
    radiodoge_packet: &[u8],
    channel: &Channel,
    profile: &crate::radio::RawProfile,
) -> Result<Vec<u8>, String> {
    let frame = wrap_radiodoge(from, id, radiodoge_packet, channel).map_err(|e| e.to_string())?;
    crate::radio::build_raw_tx(host, profile, &frame)
}

/// Build a Meshtastic text-message frame command (portnum TEXT_MESSAGE_APP).
pub fn bridge_text_command(
    host: &crate::types::NodeAddress,
    from: u32,
    id: u32,
    text: &str,
    channel: &Channel,
    profile: &crate::radio::RawProfile,
) -> Result<Vec<u8>, String> {
    let header = Header {
        to: BROADCAST,
        from,
        id,
        hop_limit: 3,
        hop_start: 3,
        want_ack: false,
        via_mqtt: false,
        channel_hash: 0,
        next_hop: 0,
        relay_node: 0,
    };
    let frame = encode_frame(
        header,
        &Data {
            portnum: PORT_TEXT_MESSAGE,
            payload: text.as_bytes().to_vec(),
        },
        channel,
    )
    .map_err(|e| e.to_string())?;
    crate::radio::build_raw_tx(host, profile, &frame)
}

/// Decode the data of a CMD_RAW_RX frame on `channel`.
pub fn bridge_decode(frame: &[u8], channel: &Channel) -> BridgeEvent {
    match decode_frame(frame, channel) {
        Ok(Some((h, d))) => match d.portnum {
            PORT_PRIVATE_APP => BridgeEvent::RadioDoge {
                from: h.from,
                packet: d.payload,
            },
            PORT_TEXT_MESSAGE => BridgeEvent::Text {
                from: h.from,
                text: String::from_utf8_lossy(&d.payload).into_owned(),
            },
            p => BridgeEvent::Other {
                from: h.from,
                portnum: p,
            },
        },
        _ => BridgeEvent::Undecodable,
    }
}

/// A random-enough packet id for Meshtastic dedupe (never 0).
pub fn new_packet_id() -> u32 {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1);
    let x = (n ^ (n >> 29)).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    ((x >> 32) as u32).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_round_trip_through_raw_tx() {
        use crate::radio::{RawProfile, RAW_PROFILE_LEN};
        use crate::types::NodeAddress;
        let ch = Channel::default_longfast();
        let host = NodeAddress::new(1, 1, 1);
        let p = RawProfile::meshtastic_longfast_us();
        let cmd = bridge_tx_command(&host, 0x1234, 7, b"doge", &ch, &p).unwrap();
        let frame = &cmd[8 + RAW_PROFILE_LEN..];
        assert_eq!(
            bridge_decode(frame, &ch),
            BridgeEvent::RadioDoge {
                from: 0x1234,
                packet: b"doge".to_vec()
            }
        );
        let t = bridge_text_command(&host, 9, 8, "hi", &ch, &p).unwrap();
        assert_eq!(
            bridge_decode(&t[8 + RAW_PROFILE_LEN..], &ch),
            BridgeEvent::Text {
                from: 9,
                text: "hi".into()
            }
        );
        assert_eq!(bridge_decode(&[1, 2, 3], &ch), BridgeEvent::Undecodable);
        assert_ne!(new_packet_id(), 0);
    }

    #[test]
    fn longfast_hash_matches_meshtastic() {
        // Meshtastic's default LongFast channel is known to hash to 8.
        assert_eq!(Channel::default_longfast().hash().unwrap(), 8);
    }

    #[test]
    fn header_roundtrip() {
        let h = Header {
            to: BROADCAST,
            from: 0x1234_5678,
            id: 42,
            hop_limit: 3,
            hop_start: 7,
            want_ack: true,
            via_mqtt: false,
            channel_hash: 8,
            next_hop: 1,
            relay_node: 2,
        };
        assert_eq!(Header::parse(&h.to_bytes()).unwrap(), h);
    }

    #[test]
    fn data_protobuf_known_bytes() {
        let d = Data {
            portnum: PORT_TEXT_MESSAGE,
            payload: b"hi".to_vec(),
        };
        assert_eq!(d.encode(), vec![0x08, 0x01, 0x12, 0x02, b'h', b'i']);
        let p = Data {
            portnum: PORT_PRIVATE_APP,
            payload: vec![],
        };
        assert_eq!(p.encode(), vec![0x08, 0x80, 0x02, 0x12, 0x00]);
        // Unknown fields (e.g. 9: emoji fixed32, 4: dest varint) are skipped.
        let mut withx = d.encode();
        withx.extend_from_slice(&[0x20, 0x05, 0x4d, 1, 2, 3, 4]);
        assert_eq!(Data::decode(&withx).unwrap(), d);
        assert!(Data::decode(&[0x12, 0x09, 1]).is_err());
    }

    #[test]
    fn wrap_unwrap_roundtrip_and_is_encrypted() {
        let ch = Channel::default_longfast();
        let pkt = [0x03, 0x01, 0x02, b'w', b'o', b'w'];
        let f = wrap_radiodoge(0xdead_beef, 7, &pkt, &ch).unwrap();
        assert_eq!(f[13], 8);
        assert!(!f[HEADER_LEN..].windows(3).any(|w| w == b"wow"));
        assert_eq!(unwrap_radiodoge(&f, &ch).unwrap(), Some(pkt.to_vec()));
        let other = Channel {
            name: "Other".into(),
            key: vec![1],
        };
        assert_eq!(unwrap_radiodoge(&f, &other).unwrap(), None);
    }

    #[test]
    fn key_rules_and_limits() {
        assert_eq!(
            Channel {
                name: "x".into(),
                key: vec![]
            }
            .expanded_key()
            .unwrap(),
            Vec::<u8>::new()
        );
        assert_eq!(
            Channel {
                name: "x".into(),
                key: vec![2]
            }
            .expanded_key()
            .unwrap()[15],
            0x02
        );
        assert!(Channel {
            name: "x".into(),
            key: vec![0; 5]
        }
        .expanded_key()
        .is_err());
        let aes256 = Channel {
            name: "Doge".into(),
            key: vec![7; 32],
        };
        let f = wrap_radiodoge(1, 2, b"much", &aes256).unwrap();
        assert_eq!(
            unwrap_radiodoge(&f, &aes256).unwrap(),
            Some(b"much".to_vec())
        );
        assert!(matches!(
            wrap_radiodoge(1, 2, &[0; 240], &aes256),
            Err(MeshError::TooLong(_))
        ));
        assert_eq!(Header::parse(&[0; 4]), Err(MeshError::TooShort));
    }
}
