//! Lightweight SPV (Simplified Payment Verification) primitives.
//!
//! This module lets the app verify that a transaction is included in the
//! Dogecoin block chain **without running a full node**, using two building
//! blocks:
//!
//! 1. **Block-header chain validation** ([`verify_header_chain`]) — parse the
//!    80-byte headers, confirm each one links to the previous by its
//!    `prev_block` hash, and (optionally) confirm each header's proof-of-work
//!    hash meets the difficulty target encoded in `nBits`.
//! 2. **Merkle-proof verification** ([`verify_merkle_proof`]) — given a txid, a
//!    merkle branch, and its index within the block, recompute the block's
//!    merkle root and compare it to the root committed in the header.
//!
//! Together these answer: *"is this txid committed to by a block header whose
//! chain I have checked?"*
//!
//! ## Proof-of-work note
//!
//! A block's **identifier hash** (used for chain linkage) is `SHA256d` of the
//! 80-byte header on every Bitcoin-derived chain, including Dogecoin — that is
//! what [`BlockHeader::hash`] computes. Dogecoin's *proof-of-work*, however, is
//! Scrypt (and, past the AuxPoW fork, merged-mined), so a fully trustless PoW
//! check is out of scope for this lightweight validator. [`hash_meets_target`]
//! therefore checks the `SHA256d` header hash against the target — which is the
//! correct PoW test for Bitcoin-style chains and a useful *consistency* check
//! for Dogecoin, but is not by itself a Scrypt/AuxPoW validation. Chain linkage
//! and merkle-proof verification are fully correct for Dogecoin.

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};

use crate::wallet::BLOCKBOOK_BASE;

/// Length of a serialized block header in bytes.
pub const BLOCK_HEADER_LEN: usize = 80;

/// A parsed 80-byte block header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockHeader {
    /// Block version.
    pub version: i32,
    /// Hash of the previous block's header (internal byte order, little-endian).
    pub prev_block: [u8; 32],
    /// Merkle root of the block's transactions (internal byte order).
    pub merkle_root: [u8; 32],
    /// Block timestamp (Unix seconds).
    pub time: u32,
    /// Compact difficulty target (`nBits`).
    pub bits: u32,
    /// PoW nonce.
    pub nonce: u32,
}

impl BlockHeader {
    /// Parse a header from exactly 80 bytes (as served on the wire / by APIs).
    pub fn parse(bytes: &[u8]) -> Result<BlockHeader> {
        if bytes.len() != BLOCK_HEADER_LEN {
            bail!(
                "block header must be {} bytes, got {}",
                BLOCK_HEADER_LEN,
                bytes.len()
            );
        }
        let version = i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        let mut prev_block = [0u8; 32];
        prev_block.copy_from_slice(&bytes[4..36]);
        let mut merkle_root = [0u8; 32];
        merkle_root.copy_from_slice(&bytes[36..68]);
        let time = u32::from_le_bytes([bytes[68], bytes[69], bytes[70], bytes[71]]);
        let bits = u32::from_le_bytes([bytes[72], bytes[73], bytes[74], bytes[75]]);
        let nonce = u32::from_le_bytes([bytes[76], bytes[77], bytes[78], bytes[79]]);
        Ok(BlockHeader {
            version,
            prev_block,
            merkle_root,
            time,
            bits,
            nonce,
        })
    }

    /// Parse a header from an 80-byte (160 hex char) hex string.
    pub fn parse_hex(hex_str: &str) -> Result<BlockHeader> {
        let bytes = hex::decode(hex_str.trim()).context("header hex decode failed")?;
        BlockHeader::parse(&bytes)
    }

    /// Serialize the header back to its 80 canonical bytes.
    pub fn serialize(&self) -> [u8; BLOCK_HEADER_LEN] {
        let mut out = [0u8; BLOCK_HEADER_LEN];
        out[0..4].copy_from_slice(&self.version.to_le_bytes());
        out[4..36].copy_from_slice(&self.prev_block);
        out[36..68].copy_from_slice(&self.merkle_root);
        out[68..72].copy_from_slice(&self.time.to_le_bytes());
        out[72..76].copy_from_slice(&self.bits.to_le_bytes());
        out[76..80].copy_from_slice(&self.nonce.to_le_bytes());
        out
    }

    /// The block hash (`SHA256d` of the serialized header), in internal byte
    /// order (little-endian).
    pub fn hash(&self) -> [u8; 32] {
        sha256d(&self.serialize())
    }

    /// The block hash as the conventional big-endian display hex string (the
    /// form block explorers show), i.e. the internal hash byte-reversed.
    pub fn hash_hex(&self) -> String {
        let mut h = self.hash();
        h.reverse();
        hex::encode(h)
    }
}

/// Double SHA-256.
fn sha256d(data: &[u8]) -> [u8; 32] {
    let first = Sha256::digest(data);
    Sha256::digest(first).into()
}

/// `true` if a compact `nBits` value is one Bitcoin Core's `SetCompact` accepts.
///
/// Two encodings are invalid and must never be expanded into a usable target:
///
/// - **Negative**, i.e. bit `0x0080_0000` set. That bit is a sign flag, not part
///   of the mantissa. Reading it as mantissa yields a target roughly 128× the
///   real one — and a *larger* target is *easier* to meet, so a forged header
///   carrying `0x1d80ffff` would have passed a difficulty check it should fail.
/// - **Overflowing**, i.e. a mantissa/exponent pair whose value does not fit in
///   256 bits.
pub fn compact_is_valid(bits: u32) -> bool {
    let size = bits >> 24;
    let word = bits & 0x007f_ffff;
    if bits & 0x0080_0000 != 0 {
        return false; // negative
    }
    if word == 0 {
        return true; // zero is representable (and expands to a zero target)
    }
    // Mirrors Core's overflow test.
    !(size > 34 || (word > 0xff && size > 33) || (word > 0xffff && size > 32))
}

/// Expand a compact `nBits` value into the full 256-bit target (big-endian bytes).
///
/// Compact form: the most-significant byte is the base-256 exponent; the low
/// three bytes are the mantissa. Mirrors Bitcoin Core's `SetCompact`.
///
/// A negative or overflowing encoding (see [`compact_is_valid`]) expands to a
/// **zero** target, which no hash can meet. That is the safe direction: the old
/// implementation ignored the sign bit and let `exponent` wrap, so
/// `nBits = 0x2080_0000` produced a target of 2^255 — a value nearly every hash
/// is below, making the proof-of-work check accept anything at all.
pub fn bits_to_target(bits: u32) -> [u8; 32] {
    let mut target = [0u8; 32];
    if !compact_is_valid(bits) {
        return target;
    }
    let exponent = (bits >> 24) & 0xff;
    let mantissa = bits & 0x007f_ffff;
    if mantissa == 0 {
        return target;
    }
    // The mantissa occupies `exponent` bytes counting from the least-significant
    // end. Place its 3 bytes so the lowest mantissa byte sits at position
    // (exponent - 1) from the big-endian right edge.
    let exp = exponent as usize;
    for (i, m_byte) in [
        ((mantissa >> 16) & 0xff) as u8,
        ((mantissa >> 8) & 0xff) as u8,
        (mantissa & 0xff) as u8,
    ]
    .into_iter()
    .enumerate()
    {
        // Byte offset from the right (least-significant) edge for this mantissa
        // byte. `checked_sub` drops bytes shifted past the right edge by a small
        // exponent; `compact_is_valid` has already excluded a large one.
        if let Some(from_right) = exp.checked_sub(1 + i) {
            if from_right < 32 {
                target[31 - from_right] = m_byte;
            }
        }
    }
    target
}

/// Returns `true` if `hash` (internal little-endian order) is numerically
/// less than or equal to the target encoded by `bits`.
///
/// See the module-level proof-of-work note about what this does and does not
/// prove on Dogecoin.
pub fn hash_meets_target(hash: &[u8; 32], bits: u32) -> bool {
    if !compact_is_valid(bits) {
        return false; // an encoding no honest header carries
    }
    let target = bits_to_target(bits);
    if target == [0u8; 32] {
        return false; // nothing can be at or below zero
    }
    // Compare big-endian. `hash` is little-endian internal order, so reverse it.
    let mut be = *hash;
    be.reverse();
    // Lexicographic comparison of equal-length big-endian byte arrays is a
    // numeric comparison.
    be.as_slice() <= target.as_slice()
}

/// Verify a chain of consecutive block headers.
///
/// - `headers` must be in ascending height order (each links to the previous).
/// - `check_pow`: if `true`, also require each header's `SHA256d` hash to meet
///   its `nBits` target (see the module-level note on Dogecoin PoW).
///
/// Returns `Ok(())` if every adjacent pair links correctly (and, when
/// requested, every header meets its target); otherwise an error describing the
/// first failing link.
pub fn verify_header_chain(headers: &[BlockHeader], check_pow: bool) -> Result<()> {
    if headers.is_empty() {
        bail!("cannot verify an empty header chain");
    }
    for (i, h) in headers.iter().enumerate() {
        if check_pow && !hash_meets_target(&h.hash(), h.bits) {
            bail!(
                "header at index {} ({}) does not meet its difficulty target",
                i,
                h.hash_hex()
            );
        }
        if i > 0 {
            let prev = &headers[i - 1];
            if h.prev_block != prev.hash() {
                bail!(
                    "header at index {} ({}) does not link to the previous header ({})",
                    i,
                    h.hash_hex(),
                    prev.hash_hex()
                );
            }
        }
    }
    Ok(())
}

/// Recompute a merkle root from a leaf txid and its inclusion branch.
///
/// - `txid_internal`: the txid in **internal** (little-endian) byte order.
/// - `branch`: sibling hashes bottom-up, each in internal byte order.
/// - `index`: the 0-based position of the tx within the block's tx list. Bit 0
///   of `index` selects the side at the lowest level, bit 1 the next, etc.
///
/// Returns the reconstructed merkle root in internal byte order, or `None` if
/// the proof is structurally invalid — see the checks inside.
pub fn merkle_root_from_branch(
    txid_internal: &[u8; 32],
    branch: &[[u8; 32]],
    index: u32,
) -> Option<[u8; 32]> {
    // A branch of `n` levels describes a tree with at most 2^n leaves, so an
    // index needing more than `n` bits does not belong to this proof. The old
    // code shifted the surplus bits away and returned a root regardless, which
    // let one branch "prove" many different positions.
    if branch.len() < 32 && (index >> branch.len()) != 0 {
        return None;
    }

    let mut acc = *txid_internal;
    let mut idx = index;
    for sibling in branch {
        // CVE-2012-2459. A Bitcoin merkle tree duplicates the last node when a
        // level has an odd number of entries, so a node paired with *itself* is
        // structurally possible — and an attacker can exploit that to build a
        // second, different transaction list with the same root. A legitimate
        // proof never needs a sibling equal to the node it is paired with.
        if *sibling == acc {
            return None;
        }
        let mut buf = [0u8; 64];
        if idx & 1 == 0 {
            // Current node is on the left.
            buf[..32].copy_from_slice(&acc);
            buf[32..].copy_from_slice(sibling);
        } else {
            // Current node is on the right.
            buf[..32].copy_from_slice(sibling);
            buf[32..].copy_from_slice(&acc);
        }
        acc = sha256d(&buf);
        idx >>= 1;
    }
    Some(acc)
}

/// Reverse a 32-byte hash — converts between display (big-endian) hex order and
/// internal (little-endian) order.
fn reverse32(mut h: [u8; 32]) -> [u8; 32] {
    h.reverse();
    h
}

/// Decode a display-order (big-endian) 32-byte hash hex string into internal
/// (little-endian) byte order.
fn display_hex_to_internal(hash_hex: &str) -> Result<[u8; 32]> {
    let bytes = hex::decode(hash_hex.trim()).context("hash hex decode failed")?;
    if bytes.len() != 32 {
        bail!("expected a 32-byte (64 hex char) hash, got {} bytes", bytes.len());
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(reverse32(arr))
}

/// Verify a merkle inclusion proof using **display-order** (block-explorer) hex
/// strings, the form APIs return.
///
/// - `txid_hex`: the transaction id (big-endian display hex).
/// - `branch_hex`: sibling hashes, bottom-up, each big-endian display hex.
/// - `index`: 0-based position of the tx in the block.
/// - `merkle_root_hex`: the block header's merkle root (big-endian display hex).
///
/// Returns `Ok(true)` if the proof reconstructs the given root, `Ok(false)` if
/// it reconstructs a different root, or an error if any input is malformed.
pub fn verify_merkle_proof(
    txid_hex: &str,
    branch_hex: &[String],
    index: u32,
    merkle_root_hex: &str,
) -> Result<bool> {
    let txid = display_hex_to_internal(txid_hex)?;
    let mut branch = Vec::with_capacity(branch_hex.len());
    for (i, s) in branch_hex.iter().enumerate() {
        branch.push(
            display_hex_to_internal(s)
                .with_context(|| format!("merkle branch element {} is invalid", i))?,
        );
    }
    let expected_root = display_hex_to_internal(merkle_root_hex)?;
    match merkle_root_from_branch(&txid, &branch, index) {
        Some(computed) => Ok(computed == expected_root),
        // A structurally invalid proof is not "a proof of a different root" — it
        // is not a proof at all. Reporting `false` rather than an error keeps the
        // caller's contract ("does this prove inclusion?") answerable.
        None => Ok(false),
    }
}

/// The confirmation status of a transaction, as reported by a Blockbook server.
///
/// This is the *practical* lightweight inclusion check the app uses today: it
/// confirms a txid is mined into a named block with a given depth, without
/// running a full node. A fully trustless proof additionally requires a merkle
/// branch and a trusted header chain (see [`verify_merkle_proof`] /
/// [`verify_header_chain`]); Blockbook's v2 REST API does not serve merkle
/// branches, so that path awaits an Electrum-style source (roadmap v0.5.x).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TxInclusion {
    /// The transaction id queried.
    pub txid: String,
    /// `true` once the tx is in a block (confirmations ≥ 1).
    pub confirmed: bool,
    /// Number of confirmations (0 while unconfirmed / in the mempool).
    pub confirmations: u32,
    /// The block height, or `None` if unconfirmed.
    pub block_height: Option<u64>,
    /// The block hash (display hex), or `None` if unconfirmed.
    pub block_hash: Option<String>,
}

/// Query a Blockbook server for a transaction's confirmation status.
///
/// Requires an internet connection. Returns [`TxInclusion`] describing whether
/// the tx is mined, how deep, and in which block.
pub async fn fetch_tx_inclusion(txid: &str) -> Result<TxInclusion> {
    let txid = txid.trim();
    if txid.len() != 64 || !txid.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("'{}' is not a valid 64-hex-character transaction id", txid);
    }

    #[derive(serde::Deserialize)]
    struct TxInfo {
        #[serde(default)]
        #[serde(rename = "blockHeight")]
        block_height: i64,
        #[serde(default)]
        #[serde(rename = "blockHash")]
        block_hash: Option<String>,
        #[serde(default)]
        confirmations: u32,
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| anyhow::anyhow!("HTTP client init failed: {}", e))?;
    let url = format!("{}/tx/{}", BLOCKBOOK_BASE, txid);
    let info: TxInfo = client
        .get(&url)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Transaction lookup failed: {}", e))?
        .error_for_status()
        .map_err(|e| {
            anyhow::anyhow!(
                "Transaction lookup failed: HTTP {}",
                e.status().map(|s| s.as_u16()).unwrap_or(0)
            )
        })?
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("Transaction response parse failed: {}", e))?;

    let confirmed = info.confirmations >= 1 && info.block_height >= 0;
    Ok(TxInclusion {
        txid: txid.to_string(),
        confirmed,
        confirmations: info.confirmations,
        block_height: if info.block_height >= 0 {
            Some(info.block_height as u64)
        } else {
            None
        },
        block_hash: if confirmed { info.block_hash } else { None },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build the Dogecoin genesis header from its known field values and confirm
    /// [`BlockHeader::hash`] reproduces the canonical genesis block hash. This
    /// exercises header parsing, serialization, and SHA256d hashing end-to-end.
    fn dogecoin_genesis() -> BlockHeader {
        // Field values from the Dogecoin genesis block.
        let merkle_root =
            display_hex_to_internal("5b2a3f53f605d62c53e62932dac6925e3d74afa5a4b459745c36d42d0ed26a69")
                .unwrap();
        BlockHeader {
            version: 1,
            prev_block: [0u8; 32],
            merkle_root,
            time: 1386325540,
            bits: 0x1e0f_fff0,
            nonce: 99943,
        }
    }

    #[test]
    fn test_dogecoin_genesis_hash() {
        let genesis = dogecoin_genesis();
        assert_eq!(
            genesis.hash_hex(),
            "1a91e3dace36e2be3bf030a65679fe821aa1d6ef92e7c9902eb318182c355691",
            "computed genesis hash must match the canonical Dogecoin genesis block hash"
        );
    }

    #[test]
    fn test_header_serialize_roundtrip() {
        let genesis = dogecoin_genesis();
        let bytes = genesis.serialize();
        assert_eq!(bytes.len(), BLOCK_HEADER_LEN);
        let reparsed = BlockHeader::parse(&bytes).expect("reparse serialized header");
        assert_eq!(reparsed, genesis);
    }

    #[test]
    fn test_parse_rejects_wrong_length() {
        assert!(BlockHeader::parse(&[0u8; 79]).is_err());
        assert!(BlockHeader::parse(&[0u8; 81]).is_err());
        assert!(BlockHeader::parse(&[0u8; 80]).is_ok());
    }

    #[test]
    fn test_bits_to_target_known_values() {
        // Bitcoin's max target (difficulty-1) is 0x1d00ffff → 0x00000000FFFF0000…0000,
        // i.e. bytes 4 and 5 (0-based, big-endian) are 0xFF.
        let target = bits_to_target(0x1d00_ffff);
        let mut expected = [0u8; 32];
        expected[4] = 0xff;
        expected[5] = 0xff;
        assert_eq!(target, expected);

        // The Dogecoin genesis nBits 0x1e0ffff0 → 0x00000FFFF0…0000.
        let target = bits_to_target(0x1e0f_fff0);
        let mut expected = [0u8; 32];
        expected[2] = 0x0f;
        expected[3] = 0xff;
        expected[4] = 0xf0;
        assert_eq!(target, expected);
    }

    #[test]
    fn test_bitcoin_genesis_sha256d_pow_meets_target() {
        // Bitcoin's PoW *is* SHA256d, so its genesis header hash must meet the
        // target encoded in nBits — a direct test of hash_meets_target against a
        // real chain. (Bitcoin genesis field values.)
        let merkle_root =
            display_hex_to_internal("4a5e1e4baab89f3a32518a88c31bc87f618f76673e2cc77ab2127b7afdeda33b")
                .unwrap();
        let btc_genesis = BlockHeader {
            version: 1,
            prev_block: [0u8; 32],
            merkle_root,
            time: 1231006505,
            bits: 0x1d00_ffff,
            nonce: 2083236893,
        };
        assert_eq!(
            btc_genesis.hash_hex(),
            "000000000019d6689c085ae165831e934ff763ae46a2a6c172b3f1b60a8ce26f"
        );
        assert!(
            hash_meets_target(&btc_genesis.hash(), btc_genesis.bits),
            "Bitcoin genesis SHA256d hash must meet its difficulty target"
        );
    }

    #[test]
    fn test_dogecoin_genesis_sha256d_does_not_meet_target() {
        // Dogecoin's PoW is Scrypt, not SHA256d, so the SHA256d header hash is NOT
        // below the nBits target. This documents the module's proof-of-work caveat:
        // hash_meets_target is a SHA256d check, valid as a Scrypt-independent
        // consistency test only.
        let genesis = dogecoin_genesis();
        assert!(
            !hash_meets_target(&genesis.hash(), genesis.bits),
            "Dogecoin genesis SHA256d hash is expected NOT to meet the target (PoW is Scrypt)"
        );
    }

    #[test]
    fn test_hash_above_target_rejected() {
        // A hash of all 0xFF is the maximum possible value and cannot meet any
        // realistic target.
        let max_hash = [0xffu8; 32];
        assert!(!hash_meets_target(&max_hash, 0x1d00_ffff));
    }

    #[test]
    fn test_header_chain_linkage() {
        // Build header B whose prev_block points at header A's hash.
        let a = dogecoin_genesis();
        let mut b = dogecoin_genesis();
        b.prev_block = a.hash();
        b.nonce = 12345;
        b.time = a.time + 60;

        assert!(verify_header_chain(&[a.clone(), b.clone()], false).is_ok());

        // Break the link.
        let mut bad = b.clone();
        bad.prev_block = [0u8; 32];
        assert!(verify_header_chain(&[a, bad], false).is_err());
    }

    #[test]
    fn test_empty_chain_rejected() {
        assert!(verify_header_chain(&[], false).is_err());
    }

    /// A 4-leaf merkle tree, verified against a proof for one leaf.
    #[test]
    fn test_merkle_proof_four_leaves() {
        // Four synthetic "txids" in internal order.
        let leaves: Vec<[u8; 32]> = (0u8..4)
            .map(|i| {
                let mut h = [0u8; 32];
                h[0] = i + 1;
                h
            })
            .collect();

        // Build the tree bottom-up.
        let hash_pair = |a: &[u8; 32], b: &[u8; 32]| -> [u8; 32] {
            let mut buf = [0u8; 64];
            buf[..32].copy_from_slice(a);
            buf[32..].copy_from_slice(b);
            sha256d(&buf)
        };
        let n01 = hash_pair(&leaves[0], &leaves[1]);
        let n23 = hash_pair(&leaves[2], &leaves[3]);
        let root = hash_pair(&n01, &n23);

        // Proof for leaf index 2: siblings are leaf[3] (level 0) then n01 (level 1).
        let branch = [leaves[3], n01];
        let computed = merkle_root_from_branch(&leaves[2], &branch, 2);
        assert_eq!(computed, Some(root));

        // Proof for leaf index 0: siblings are leaf[1] then n23.
        let branch0 = [leaves[1], n23];
        assert_eq!(merkle_root_from_branch(&leaves[0], &branch0, 0), Some(root));

        // A wrong index must not reconstruct the root.
        assert_ne!(merkle_root_from_branch(&leaves[2], &branch, 0), Some(root));

        // An index that needs more bits than the branch has levels does not
        // belong to this proof at all.
        assert_eq!(merkle_root_from_branch(&leaves[2], &branch, 4), None);
        assert_eq!(merkle_root_from_branch(&leaves[2], &branch, u32::MAX), None);

        // CVE-2012-2459: a sibling equal to the node it is paired with is the
        // duplicated-node shape used to forge a second transaction list with the
        // same root. It is never needed by a legitimate proof.
        assert_eq!(merkle_root_from_branch(&leaves[0], &[leaves[0]], 0), None);
        // Also at a higher level: n23 paired with itself.
        assert_eq!(merkle_root_from_branch(&leaves[2], &[leaves[3], n23], 2), None);
    }

    /// Compact difficulty values Bitcoin Core rejects must never expand into a
    /// target a hash can meet.
    ///
    /// The sign bit used to be read as mantissa, so `0x1d80ffff` produced a
    /// target ~128x the real one — and a bigger target is *easier* to meet. The
    /// exponent was also allowed to wrap, so `0x20800000` gave a target of 2^255,
    /// which nearly every hash is below.
    #[test]
    fn test_invalid_compact_bits_are_rejected() {
        for bad in [0x0080_0000u32, 0x2080_0000, 0x1d80_ffff, 0xff00_0001, 0xff7f_ffff] {
            assert!(!compact_is_valid(bad), "0x{:08x} should be rejected", bad);
            assert_eq!(bits_to_target(bad), [0u8; 32], "0x{:08x} must expand to zero", bad);
            assert!(
                !hash_meets_target(&[0u8; 32], bad),
                "0x{:08x} must not let even a zero hash pass",
                bad
            );
        }

        // Real values from the chain still work exactly as before.
        for good in [0x1d00_ffffu32, 0x1b04_04cb, 0x1e0f_fff0, 0x2100_ffff] {
            assert!(compact_is_valid(good), "0x{:08x} should be accepted", good);
            assert_ne!(bits_to_target(good), [0u8; 32]);
        }
    }

    /// A single-transaction block: the merkle root equals the coinbase txid, and
    /// an empty branch verifies. (Dogecoin genesis is exactly this case.)
    #[test]
    fn test_merkle_proof_single_tx_via_display_hex() {
        let txid = "5b2a3f53f605d62c53e62932dac6925e3d74afa5a4b459745c36d42d0ed26a69";
        // Empty branch, index 0, root == txid.
        let ok = verify_merkle_proof(txid, &[], 0, txid).expect("verify single-tx proof");
        assert!(ok);

        // A mismatched root must fail closed.
        let other = "0000000000000000000000000000000000000000000000000000000000000001";
        assert!(!verify_merkle_proof(txid, &[], 0, other).unwrap());
    }

    #[test]
    fn test_verify_merkle_proof_rejects_bad_hex() {
        assert!(verify_merkle_proof("xyz", &[], 0, "abc").is_err());
    }
}
