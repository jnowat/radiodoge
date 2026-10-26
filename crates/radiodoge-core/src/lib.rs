//! # radiodoge-core
//!
//! Pure Rust library providing the RadioDoge protocol stack:
//!
//! - [`types`] — shared data types (NodeAddress, WalletInfo, packets, stats)
//! - [`wallet`] — Dogecoin keypair generation and transaction encoding (pure Rust, no FFI)
//! - [`radio`] — LoRa packet protocol encoding/decoding
//! - [`serial`] — cross-platform serial port management with async Tokio runtime
//! - [`backend`] — chain data sources (Dogecoin Core RPC, BlockCypher, Blockbook) with fallback
//! - [`spv`] — lightweight SPV header-chain and merkle-proof verification
//!
//! This crate has **no Tauri dependency** — it is shared between:
//! - `radiodoge-gui` (Tauri 2 desktop GUI)
//! - `radiodoge-cli` (headless CLI replacing RadioDogeSharp and serdog)
//! - Future Android / embedded targets
//!
//! Building with `default-features = false` keeps only [`radio`] and [`types`]
//! (no Tokio, serial, crypto or HTTP), which is what the WebAssembly packet
//! inspector uses.

#[cfg(feature = "native")]
pub mod backend;
#[cfg(feature = "native")]
pub mod gateway_stats;
#[cfg(feature = "native")]
pub mod geo;
#[cfg(feature = "native")]
pub mod store_forward;
#[cfg(feature = "native")]
pub mod qr;
pub mod radio;
#[cfg(feature = "native")]
pub mod serial;
#[cfg(feature = "native")]
pub mod spv;
pub mod types;
#[cfg(feature = "native")]
pub mod wallet;
