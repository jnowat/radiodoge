//! RadioDoge CLI — Rust port of RadioDogeSharp (C#) and serdog (C).
//!
//! Provides a complete headless interface to the Heltec ESP32 LoRa device:
//! send Dogecoin, receive packets, generate wallets, and run as a daemon.
//! No GUI required — works on Windows, Linux, macOS, and eventually Raspberry Pi.
//!
//! Much CLI. Very terminal. Such Rust. Wow. 🐕

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use radiodoge_core::gateway_stats::{GatewayEventKind, GatewayStats};
use radiodoge_core::store_forward::{parse_node, Outbox, PendingBroadcast};
use radiodoge_core::{radio, spv, wallet};
use radiodoge_core::serial::SerialManager;
use radiodoge_core::tx_reply::{self, RecentSends, TxReply};
use radiodoge_core::types::{IncomingPacket, NodeAddress};

mod cli;
use cli::*;

mod daemon;
use daemon::*;

mod meshtastic_cmd;
use meshtastic_cmd::*;

// ─── Main ────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialise logging
    let log_level = if cli.verbose { "debug" } else { "info" };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(log_level))
        .init();

    if cli.backends.is_some() || cli.rpc_url.is_some() || cli.rpc_user.is_some() || cli.rpc_cookie.is_some() {
        let spec = cli.backends.clone()
            .or_else(|| std::env::var("RADIODOGE_BACKENDS").ok())
            .unwrap_or_else(|| "core,blockcypher".to_string());
        let (url, user, cookie) = (cli.rpc_url.clone(), cli.rpc_user.clone(), cli.rpc_cookie.clone());
        let env = move |k: &str| match k {
            "RADIODOGE_RPC_URL" if url.is_some() => url.clone(),
            "RADIODOGE_RPC_USER" if user.is_some() => user.clone(),
            "RADIODOGE_RPC_COOKIE" if cookie.is_some() => cookie.clone(),
            _ => std::env::var(k).ok(),
        };
        radiodoge_core::backend::set_backends(Some(radiodoge_core::backend::parse_backends(&spec, &env)?));
    }

    let result = match cli.command {
        Commands::Ports => cmd_ports(),
        Commands::Wallet { cmd } => cmd_wallet(cmd),
        Commands::Meshtastic { cmd: MeshtasticCommands::Bridge { port, listen, send_hex, text, from, freq_khz, power, channel } } => {
            cmd_meshtastic_bridge(&port, listen, send_hex, text, &from, freq_khz, power, channel).await
        }
        Commands::Meshtastic { cmd } => cmd_meshtastic(cmd),
        Commands::Send { port, to_address, amount, memo, wif, fee, gateway, timeout, force } => {
            cmd_send(&port, &to_address, amount, memo.as_deref(), wif.as_deref(), fee, gateway.as_deref(), timeout, force).await
        }
        Commands::Receive { port, timeout } => cmd_receive(&port, timeout).await,
        Commands::Ping { port } => cmd_ping(&port).await,
        Commands::Discover { port, listen } => cmd_discover(&port, listen).await,
        Commands::Info { port, expect } => cmd_info(&port, expect.as_deref()).await,
        Commands::SetAddress { port, address } => cmd_set_address(&port, &address).await,
        Commands::Message { port, to, text, listen } => cmd_message(&port, &to, &text, listen).await,
        Commands::LoraPing { port, to, timeout } => cmd_lora_ping(&port, &to, timeout).await,
        Commands::LoraBalance { port, address, gateway, timeout } => cmd_lora_balance(&port, &address, gateway.as_deref(), timeout).await,
        Commands::SendTxHex { port, file, gateway, timeout, force } => cmd_send_tx_hex(&port, &file, gateway.as_deref(), timeout, force).await,
        Commands::SignTx { to_address, amount, out, fee } => cmd_sign_tx(&to_address, amount, &out, fee).await,
        Commands::Connect { port } => cmd_connect(&port).await,
        Commands::Daemon { port, stats_file, outbox_file } => cmd_daemon(&port, stats_file, outbox_file).await,
        Commands::Balance { address } => cmd_balance(&address).await,
        Commands::VerifyTx { txid } => cmd_verify_tx(&txid).await,
        Commands::Broadcast { wif, to_address, amount, fee } => cmd_broadcast(&wif, &to_address, amount, fee).await,
        Commands::Fee { from, amount } => cmd_fee(from.as_deref(), amount).await,
    };
    if let Err(e) = &result {
        // `{:#}` keeps the whole context chain on one line.
        let raw = format!("{:#}", e);
        eprintln!("❌ {}", raw);
        if let Some(hint) = radiodoge_core::errors::hint_for(&raw) {
            eprintln!("💡 {}", hint);
        }
        std::process::exit(1);
    }
    Ok(())
}

async fn cmd_fee(from: Option<&str>, amount: Option<f64>) -> Result<()> {
    let rate = wallet::network_fee_rate().await;
    println!("Network fee rate: {:.8} DOGE per started kB", rate as f64 / 1e8);
    if let (Some(addr), Some(amt)) = (from, amount) {
        if !wallet::is_valid_address(addr) {
            anyhow::bail!("Invalid Dogecoin address: {}", addr);
        }
        let est = wallet::estimate_fee(addr, amt).await?;
        println!(
            "Sending {:.8} DOGE from {}: ~{} B, {} input(s), fee {:.8} DOGE",
            amt, addr, est.size_bytes, est.inputs, est.fee_doge
        );
    }
    Ok(())
}

// ─── Command Implementations ─────────────────────────────────────────────────

/// List all available serial ports.
fn cmd_ports() -> Result<()> {
    let ports = SerialManager::list_ports();
    if ports.is_empty() {
        println!("No serial ports found. Is the Heltec connected via USB? Very sad. 😢");
    } else {
        println!("🐕 Available serial ports:");
        for p in &ports {
            println!("  • {}", p);
        }
        println!("\nUse one of these with -p / --port");
    }
    Ok(())
}

/// Wallet sub-commands.
fn cmd_wallet(cmd: WalletCommands) -> Result<()> {
    match cmd {
        WalletCommands::Generate => {
            let w = wallet::generate_keypair().context("Failed to generate keypair")?;
            println!("🐕 New Dogecoin Wallet — SAVE THIS PRIVATELY!\n");
            println!("  Address (share this):     {}", w.address);
            println!("  Public Key (hex):          {}", w.public_key_hex);
            println!("  Private Key (WIF, SECRET): {}", w.private_key_wif);
            println!("\n⚠️  The private key is shown ONCE. Write it down. Lose it = lose DOGE.");
        }
        WalletCommands::Mnemonic => {
            let mnemonic = wallet::generate_mnemonic().context("Failed to generate mnemonic")?;
            let phrase = mnemonic.to_string();
            let w = wallet::wallet_from_mnemonic(&phrase).context("Failed to derive wallet")?;
            println!("🌱 New Dogecoin Wallet with Recovery Phrase — WRITE THIS DOWN OFFLINE!\n");
            println!("  Recovery Phrase (12 words, SECRET):");
            for (i, word) in phrase.split_whitespace().enumerate() {
                println!("    {:2}. {}", i + 1, word);
            }
            println!();
            println!("  Derived at: m/44'/3'/0'/0/0 (Dogecoin BIP44)");
            println!("  Address (share this):     {}", w.address);
            println!("  Public Key (hex):          {}", w.public_key_hex);
            println!("  Private Key (WIF, SECRET): {}", w.private_key_wif);
            println!("\n⚠️  Store the recovery phrase OFFLINE. Anyone with it controls your DOGE.");
        }
        WalletCommands::ImportMnemonic { phrase } => {
            let w = wallet::wallet_from_mnemonic(&phrase)
                .context("Failed to derive wallet from mnemonic")?;
            println!("✅ Wallet restored from recovery phrase (m/44'/3'/0'/0/0)\n");
            println!("  Address (share this):     {}", w.address);
            println!("  Public Key (hex):          {}", w.public_key_hex);
            println!("  Private Key (WIF, SECRET): {}", w.private_key_wif);
        }
        WalletCommands::ImportWif { wif } => {
            let w = wallet::import_wif(&wif).context("Failed to import WIF key")?;
            println!("✅ Wallet imported from WIF key\n");
            println!("  Address (share this):     {}", w.address);
            println!("  Public Key (hex):          {}", w.public_key_hex);
            println!("  Private Key (WIF, SECRET): {}", w.private_key_wif);
        }
        WalletCommands::Validate { address } => {
            if wallet::is_valid_address(&address) {
                println!("✅ '{}' is a valid Dogecoin address! Much valid. Wow.", address);
            } else {
                println!("❌ '{}' is NOT a valid Dogecoin address.", address);
                println!("   Valid addresses start with 'D' and are 34 characters long.");
            }
        }
    }
    Ok(())
}

/// Send a Dogecoin transaction over LoRa.
#[allow(clippy::too_many_arguments)]
async fn cmd_send(port: &str, to: &str, amount: f64, memo: Option<&str>, wif: Option<&str>, fee: Option<f64>, gateway: Option<&str>, timeout: u64, force: bool) -> Result<()> {
    if !wallet::is_valid_address(to) {
        anyhow::bail!("'{}' is not a valid Dogecoin address (must start with 'D')", to);
    }
    if amount <= 0.0 {
        anyhow::bail!("Amount must be > 0 DOGE");
    }

    if let Some(key) = wif {
        println!("[1/4] 🔑 Signing P2PKH transaction (fetching UTXOs from chain backends)...");
        let (raw, paid) = wallet::build_signed_transaction_with_fee(key, to, amount, fee)
            .await
            .context("Transaction signing failed")?;
        println!("      {:.8} DOGE → {} + {:.8} fee ({} bytes)", amount, to, paid, raw.len());
        return radio_signed_tx(port, &raw, gateway, timeout, force).await;
    }

    println!("🐕 Connecting to {} ...", port);
    let manager = Arc::new(SerialManager::new());
    let on_packet = Arc::new(|_pkt: IncomingPacket| {});
    manager.connect(port, on_packet).await
        .with_context(|| format!("Failed to open serial port {}", port))?;

    // Build payload: real signed P2PKH tx when --wif supplied, legacy stub otherwise.
    let payload = if let Some(key) = wif {
        println!("🔑 Signing P2PKH transaction (fetching UTXOs from chain backends)...");
        let (raw, paid) = wallet::build_signed_transaction_with_fee(key, to, amount, fee)
            .await
            .context("Transaction signing failed")?;
        println!("   Fee: {:.8} DOGE ({} bytes)", paid, raw.len());
        raw
    } else {
        println!("✅ Connected! Encoding stub transaction...");
        wallet::encode_transaction_payload(to, amount, memo)
            .context("Failed to encode transaction")?
    };

    // Which framing the board can accept depends on its firmware, so ask before
    // deciding. `build_tx_frames` refuses anything the board would mis-frame
    // rather than transmitting a transaction no receiver can reconstruct.
    let firmware = manager.ensure_firmware_version().await;
    let src = manager.refresh_node_address(1500).await;
    let dst = NodeAddress::broadcast();

    let frames = match radio::build_tx_frames(
        &src,
        &dst,
        radio::CMD_DOGE_TX,
        &payload,
        firmware.as_deref(),
    ) {
        Ok(f) => f,
        Err(msg) => {
            manager.disconnect().await.ok();
            anyhow::bail!("{}", msg);
        }
    };

    if frames.len() > 1 {
        println!(
            "📦 {} bytes → {} LoRa frames (each acknowledged before the next is sent)…",
            payload.len(),
            frames.len()
        );
    }
    if let Err(e) = manager.send_frames(frames).await {
        manager.disconnect().await.ok();
        return Err(e).context("Failed to send transaction over LoRa");
    }

    if wif.is_some() {
        println!("✅ Signed tx sent! {:.8} DOGE → {} via LoRa 🐕🌙", amount, to);
    } else {
        let memo_note = memo.map(|m| format!(" [{}]", m)).unwrap_or_default();
        println!("✅ Sent! {:.8} DOGE → {}{} via LoRa 🐕🌙", amount, to, memo_note);
    }

    manager.disconnect().await.ok();
    Ok(())
}

/// Listen for incoming LoRa packets.
async fn cmd_receive(port: &str, timeout_secs: u64) -> Result<()> {
    println!("🐕 Connecting to {} and listening for packets...", port);
    if timeout_secs > 0 {
        println!("  (will stop after {} seconds; Ctrl-C to exit early)", timeout_secs);
    } else {
        println!("  (running forever; Ctrl-C to exit)");
    }

    let manager = Arc::new(SerialManager::new());

    let on_packet = Arc::new(|pkt: IncomingPacket| {
        let hop_note = if pkt.hops > 0 {
            format!(" | hops={}", pkt.hops)
        } else {
            String::new()
        };
        println!(
            "[{}] 📻 {} → {} | cmd=0x{:02X} | rssi={}{} | {}",
            pkt.timestamp,
            pkt.source.to_display_string(),
            pkt.destination.to_display_string(),
            pkt.command,
            pkt.rssi,
            hop_note,
            pkt.decoded.unwrap_or_else(|| format!("raw={}", pkt.payload_hex)),
        );
    });

    manager.connect(port, on_packet).await
        .with_context(|| format!("Failed to open serial port {}", port))?;

    println!("✅ Listening...\n");

    if timeout_secs > 0 {
        tokio::time::sleep(Duration::from_secs(timeout_secs)).await;
    } else {
        // Block until Ctrl-C
        tokio::signal::ctrl_c().await.context("Failed to listen for Ctrl-C")?;
    }

    println!("\n🐕 Done receiving. Disconnecting...");
    manager.disconnect().await.ok();
    Ok(())
}

fn parse_node_addr(s: &str) -> Result<NodeAddress> {
    let parts: Vec<&str> = s.trim().split('.').collect();
    if parts.len() != 3 {
        anyhow::bail!("'{}' is not a node address (expected Region.Community.Node, e.g. 10.0.2)", s);
    }
    let p = |x: &str| x.parse::<u8>().with_context(|| format!("'{}' is not 0-255", x));
    Ok(NodeAddress::new(p(parts[0])?, p(parts[1])?, p(parts[2])?))
}

fn print_packet(pkt: &IncomingPacket) {
    println!(
        "[{}] 📻 {} → {} | cmd=0x{:02X} | rssi={} | {}",
        pkt.timestamp,
        pkt.source.to_display_string(),
        pkt.destination.to_display_string(),
        pkt.command,
        pkt.rssi,
        pkt.decoded.clone().unwrap_or_else(|| format!("raw={}", pkt.payload_hex)),
    );
}

async fn open_board(port: &str) -> Result<Arc<SerialManager>> {
    let manager = Arc::new(SerialManager::new());
    manager.connect(port, Arc::new(|_: IncomingPacket| {})).await
        .with_context(|| format!("Failed to open serial port {}", port))?;
    manager.refresh_node_address(1500).await;
    Ok(manager)
}

/// Set the board's node address.
async fn cmd_set_address(port: &str, address: &str) -> Result<()> {
    let new_addr = parse_node_addr(address)?;
    if new_addr == NodeAddress::broadcast() {
        anyhow::bail!("255.255.255 is the broadcast address");
    }
    let manager = open_board(port).await?;
    let old = manager.get_node_address().await;
    println!("Current address: {}", old.to_display_string());
    manager.send_raw(radio::build_set_node_addr(&old, &new_addr)).await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let ok = manager.verify_node_address_set(&new_addr, 3000).await;
    manager.disconnect().await.ok();
    if ok {
        println!("✅ Address set to {} (stored on the board)", new_addr.to_display_string());
        Ok(())
    } else {
        anyhow::bail!("Board did not confirm address {}", new_addr.to_display_string())
    }
}

/// Send a text message over LoRa, optionally listening for replies.
async fn cmd_message(port: &str, to: &str, text: &str, listen: u64) -> Result<()> {
    let dst = parse_node_addr(to)?;
    let manager = open_board(port).await?;
    let mut rx = manager.subscribe_packets();
    let src = manager.get_node_address().await;
    // The firmware only relays a desktop-format MESSAGE (0x03) itself when it is in
    // gateway mode, so wrap each desktop-format frame in the legacy
    // HOST_FORMED_PACKET command (0x68 = [0x68, len, bytes...]), which any board
    // transmits verbatim. Payloads over one packet use the desktop multipart layout.
    let frames: Vec<Vec<u8>> = if text.len() <= radio::MAX_SINGLE_PAYLOAD_LEN {
        vec![radio::build_message(&src, &dst, text)]
    } else {
        radio::try_build_multipart_packets(&src, &dst, radio::CMD_MESSAGE, text.as_bytes())
            .map_err(|e| anyhow::anyhow!(e))?
    };
    println!("✉️  {} → {}: {} bytes in {} frame(s)", src.to_display_string(), dst.to_display_string(), text.len(), frames.len());
    for (i, f) in frames.iter().enumerate() {
        let mut wrapped = vec![0x68u8, f.len() as u8];
        wrapped.extend_from_slice(f);
        manager.send_raw(wrapped).await.with_context(|| format!("Failed to send frame {}", i + 1))?;
        // LoRa airtime for ~200 bytes at the default SF is well under a second;
        // leave room for the board's own TX-and-wait before the next frame.
        tokio::time::sleep(Duration::from_millis(1500)).await;
    }
    println!("✅ Handed to the board for LoRa transmission");
    if listen > 0 {
        let _ = tokio::time::timeout(Duration::from_secs(listen), async {
            while let Ok(pkt) = rx.recv().await {
                if pkt.source != src { print_packet(&pkt); }
            }
        }).await;
    }
    manager.disconnect().await.ok();
    Ok(())
}

/// Over-the-air ping: wait for the remote node's ACK (RECEIVED_ACK, 0x29).
async fn cmd_lora_ping(port: &str, to: &str, timeout: u64) -> Result<()> {
    let dst = parse_node_addr(to)?;
    let manager = open_board(port).await?;
    let mut rx = manager.subscribe_packets();
    let src = manager.get_node_address().await;
    let t0 = std::time::Instant::now();
    // Legacy PING_REQUEST: [0x03, 3, dst_r, dst_c, dst_n] — transmits a LoRa PING;
    // the desktop CMD_PING (0x02) is answered locally by the board and never aired.
    manager.send_raw(vec![0x03, 3, dst.region, dst.community, dst.node]).await?;
    println!("📡 PING {} → {} over LoRa…", src.to_display_string(), dst.to_display_string());
    let got = tokio::time::timeout(Duration::from_secs(timeout), async {
        while let Ok(pkt) = rx.recv().await {
            log::debug!("pkt cmd=0x{:02X} from {}", pkt.command, pkt.source.to_display_string());
            if pkt.command == 0x29 { return Some(pkt); }
        }
        None
    }).await.ok().flatten();
    manager.disconnect().await.ok();
    match got {
        Some(p) => { println!("✅ ACK after {} ms (rssi {} dBm)", t0.elapsed().as_millis(), p.rssi); Ok(()) }
        None => anyhow::bail!("No LoRa ACK from {} within {} s", dst.to_display_string(), timeout),
    }
}

/// Request a balance from a gateway over LoRa.
async fn cmd_lora_balance(port: &str, address: &str, gateway: Option<&str>, timeout: u64) -> Result<()> {
    let dst = match gateway { Some(g) => parse_node_addr(g)?, None => NodeAddress::broadcast() };
    if !wallet::is_valid_address(address) {
        anyhow::bail!("'{}' is not a valid Dogecoin address", address);
    }
    let manager = open_board(port).await?;
    let mut rx = manager.subscribe_packets();
    let src = manager.get_node_address().await;
    manager.send_raw(radio::build_request_balance_to(&src, &dst, address)).await?;
    println!("📡 Balance request for {} sent over LoRa; waiting for a gateway…", address);
    let reply = tokio::time::timeout(Duration::from_secs(timeout), async {
        while let Ok(pkt) = rx.recv().await {
            let text = pkt.decoded.clone().unwrap_or_default();
            log::debug!("pkt cmd=0x{:02X} from {}: {}", pkt.command, pkt.source.to_display_string(), text);
            let raw = String::from_utf8_lossy(&hex::decode(&pkt.payload_hex).unwrap_or_default()).to_string();
            if let Some(i) = raw.find("BAL:") { return Some((pkt.source.clone(), raw[i..].trim_end_matches('\0').to_string())); }
        }
        None
    }).await.ok().flatten();
    manager.disconnect().await.ok();
    match reply {
        Some((gw, t)) => {
            let koinus: Option<u64> = t[4..].trim().split(|c: char| !c.is_ascii_digit()).next().and_then(|n| n.parse().ok());
            match koinus {
                Some(k) => println!("💰 Gateway {} reports {:.8} DOGE ({} koinus)", gw.to_display_string(), k as f64 / 1e8, k),
                None => println!("💬 Gateway {} replied: {}", gw.to_display_string(), t),
            }
            Ok(())
        }
        None => anyhow::bail!("No balance reply within {} s", timeout),
    }
}

/// Sign without broadcasting.
async fn cmd_sign_tx(to: &str, amount: f64, out: &str, fee: Option<f64>) -> Result<()> {
    if !wallet::is_valid_address(to) {
        anyhow::bail!("'{}' is not a valid Dogecoin address", to);
    }
    // Key source: RADIODOGE_WIF, or RADIODOGE_MNEMONIC_FILE (a file whose
    // first 12/24-word line is the BIP39 phrase). Neither is ever printed.
    let wif = match std::env::var("RADIODOGE_WIF") {
        Ok(w) => w,
        Err(_) => {
            let path = std::env::var("RADIODOGE_MNEMONIC_FILE")
                .context("Set RADIODOGE_WIF or RADIODOGE_MNEMONIC_FILE")?;
            let body = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path))?;
            let phrase = body.lines().map(str::trim)
                .find(|l| { let n = l.split_whitespace().count(); (n == 12 || n == 24) && wallet_mnemonic_ok(l) })
                .context("no valid 12/24-word mnemonic line found")?;
            let w = wallet::wallet_from_mnemonic(phrase)?;
            println!("   from:  {}", w.address);
            w.private_key_wif
        }
    };
    let (raw, fee) = match fee {
        Some(f) => (wallet::build_signed_transaction(wif.trim(), to, amount, f).await?, f),
        None => wallet::build_signed_transaction_auto_fee(wif.trim(), to, amount).await?,
    };
    let hex_tx = hex::encode(&raw);
    std::fs::write(out, &hex_tx).with_context(|| format!("writing {}", out))?;
    println!("✅ Signed (NOT broadcast): {:.8} DOGE → {} + {:.8} fee", amount, to, fee);
    println!("   txid:  {}", wallet::compute_txid(&raw));
    println!("   size:  {} bytes ({} LoRa frame(s))", raw.len(), raw.len().div_ceil(radio::MULTIPART_CHUNK_LEN).max(1));
    println!("   saved: {}", out);
    Ok(())
}

fn wallet_mnemonic_ok(line: &str) -> bool {
    wallet::wallet_from_mnemonic(line).is_ok()
}

/// Radio a pre-signed transaction to a gateway and wait for its TX_ACK.
async fn cmd_send_tx_hex(port: &str, file: &str, gateway: Option<&str>, timeout: u64, force: bool) -> Result<()> {
    let hex_tx = std::fs::read_to_string(file).with_context(|| format!("reading {}", file))?;
    let raw = hex::decode(hex_tx.trim()).context("file is not valid hex")?;
    let (_from, outs) = wallet::verify_signed_tx(&raw).context("not a valid signed transaction")?;
    println!("[1/4] 🔎 verified signed tx ({} bytes)", raw.len());
    for (v, a) in &outs { println!("      out {:.8} DOGE → {}", *v as f64 / 1e8, a); }
    radio_signed_tx(port, &raw, gateway, timeout, force).await
}

/// Where the sender's duplicate-broadcast guard is kept.
fn sent_ledger_path() -> std::path::PathBuf {
    if let Ok(p) = std::env::var("RADIODOGE_SENT_LEDGER") { return p.into(); }
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_else(|_| ".".into());
    std::path::Path::new(&home).join(".radiodoge").join("sent-txids.json")
}

/// Shared by `send --wif` and `send-tx-hex`: duplicate guard, frame + radio the
/// transaction with progress, then wait for the gateway's ACK / QUEUED / NACK.
async fn radio_signed_tx(port: &str, raw: &[u8], gateway: Option<&str>, timeout: u64, force: bool) -> Result<()> {
    let txid = wallet::compute_txid(raw);
    println!("      txid {}", txid);
    let ledger_path = sent_ledger_path();
    let mut ledger = RecentSends::from_json(&std::fs::read_to_string(&ledger_path).unwrap_or_default());
    if let Some(age) = ledger.recently_sent(&txid, unix_now()) {
        if !force {
            anyhow::bail!(
                "this txid was radioed {} min ago — check {} before resending (pass --force to send it again)",
                age / 60, tx_reply::explorer_url(&txid)
            );
        }
        println!("⚠️  --force: radioing a txid already sent {} min ago", age / 60);
    }
    let dst = match gateway { Some(g) => parse_node_addr(g)?, None => NodeAddress::broadcast() };
    let manager = open_board(port).await?;
    let mut rx = manager.subscribe_packets();
    let firmware = manager.ensure_firmware_version().await;
    let src = manager.get_node_address().await;
    let frames = match radio::build_tx_frames(&src, &dst, radio::CMD_DOGE_TX, raw, firmware.as_deref()) {
        Ok(f) => f,
        Err(m) => { manager.disconnect().await.ok(); anyhow::bail!("{}", m) }
    };
    let n = frames.len();
    println!("[2/4] 📦 {} frame(s), {} → {}", n, src.to_display_string(), dst.to_display_string());
    let t0 = std::time::Instant::now();
    let sent = manager.send_frames_with_progress(frames, move |done, total| {
        if total > 1 { println!("      frame {}/{} on air", done, total); }
    }).await;
    if let Err(e) = sent {
        manager.disconnect().await.ok();
        return Err(e).context("the board did not take every frame — nothing complete was aired");
    }
    ledger.record(&txid, unix_now());
    if let Some(dir) = ledger_path.parent() { std::fs::create_dir_all(dir).ok(); }
    std::fs::write(&ledger_path, ledger.to_json()).ok();
    println!("[3/4] 📡 on air in {} ms; waiting up to {} s for the gateway…", t0.elapsed().as_millis(), timeout);
    let want = txid.clone();
    let me = src.clone();
    let reply = tokio::time::timeout(Duration::from_secs(timeout), async {
        while let Ok(pkt) = rx.recv().await {
            let text = String::from_utf8_lossy(&hex::decode(&pkt.payload_hex).unwrap_or_default()).to_string();
            // Ignore other senders' replies on a shared channel: the txid must
            // match, unless the reply is addressed to this node specifically.
            let parsed = TxReply::parse(&text).filter(|r| r.txid() == want || pkt.destination == me);
            match parsed {
                // QUEUED is progress, not the end: keep listening for the ACK.
                Some(TxReply::Queued { .. }) => {
                    println!("      ⏳ {} queued it (no internet yet); it will retry", pkt.source.to_display_string());
                }
                Some(r) => return Some((pkt.source.clone(), r)),
                None => {}
            }
        }
        None
    }).await.ok().flatten();
    manager.disconnect().await.ok();
    let link = tx_reply::explorer_url(&txid);
    match reply {
        Some((gw, TxReply::Ack { txid: id })) => {
            println!("[4/4] ✅ TX_ACK from {} after {} ms", gw.to_display_string(), t0.elapsed().as_millis());
            println!("      txid {}", id);
            println!("      {}", tx_reply::explorer_url(&id));
            if id != txid { println!("⚠️  ACK txid differs from the local txid {}", txid); }
            Ok(())
        }
        Some((gw, TxReply::Nack { txid: id, reason })) => {
            anyhow::bail!("TX_NACK from {} for {}: {} — the network rejected it; do not resend the same transaction", gw.to_display_string(), id, reason)
        }
        Some((_, TxReply::Queued { .. })) | None => anyhow::bail!(
            "No TX_ACK within {} s. The transaction may still be queued or broadcast — check {} before any retry",
            timeout, link
        ),
    }
}

/// Read-only board identity: node address, firmware, and capability flags.
async fn cmd_info(port: &str, expect: Option<&str>) -> Result<()> {
    let want = expect.map(parse_node_addr).transpose()?;
    let manager = open_board(port).await?;
    let fw = manager.ensure_firmware_version().await;
    let addr = manager.get_node_address().await;
    manager.disconnect().await.ok();
    println!("📍 node address: {}", addr.to_display_string());
    println!("🧩 firmware:     {}", fw.as_deref().unwrap_or("unknown (no reply)"));
    println!("   multipart TX: {}", if radio::firmware_supports_multipart(fw.as_deref()) { "yes" } else { "no" });
    println!("   raw LoRa:     {}", if radio::firmware_supports_raw(fw.as_deref()) { "yes" } else { "no" });
    if let Some(w) = want {
        if w != addr {
            anyhow::bail!("expected node address {} but the board reports {}", w.to_display_string(), addr.to_display_string());
        }
        println!("✅ address matches {}", w.to_display_string());
    }
    Ok(())
}

/// Broadcast a discovery PING and list the nodes that answer.
async fn cmd_discover(port: &str, listen: u64) -> Result<()> {
    let listen = listen.clamp(1, 30);
    println!("🐕 Connecting to {} ...", port);
    let manager = Arc::new(SerialManager::new());
    let on_packet = Arc::new(|_: IncomingPacket| {});
    manager.connect(port, on_packet).await
        .with_context(|| format!("Failed to open serial port {}", port))?;
    let me = manager.get_node_address().await;
    println!("📡 Discovering from {} (listening {} s)...", me.to_display_string(), listen);
    let found = manager.discover_nodes(std::time::Duration::from_secs(listen)).await?;
    if found.is_empty() {
        println!("No nodes answered. Nodes need firmware with discovery support and the same LoRa settings.");
    } else {
        println!("{:<14} {:>9}", "NODE", "RSSI");
        for n in &found {
            let rssi = if n.rssi == 0 { "n/a".to_string() } else { format!("{} dBm", n.rssi) };
            println!("{:<14} {:>9}", n.address.to_display_string(), rssi);
        }
        println!("{} node(s) found", found.len());
    }
    manager.disconnect().await.ok();
    Ok(())
}

/// Ping the Heltec device.
async fn cmd_ping(port: &str) -> Result<()> {
    println!("🐕 Connecting to {} ...", port);
    let manager = Arc::new(SerialManager::new());
    let on_packet = Arc::new(|_: IncomingPacket| {});
    manager.connect(port, on_packet).await
        .with_context(|| format!("Failed to open serial port {}", port))?;

    println!("Sending PING...");
    if manager.ping().await {
        println!("✅ PONG received! Device is alive. Much responsive. Wow. 🐕");
    } else {
        println!("❌ No response within 1500 ms. Device may be offline or not running RadioDoge firmware.");
    }

    manager.disconnect().await.ok();
    Ok(())
}

/// Interactive REPL — replaces RadioDogeSharp's console menu.
async fn cmd_connect(port: &str) -> Result<()> {
    println!("🐕 RadioDoge Interactive Mode");
    println!("Connecting to {} ...", port);

    let manager = Arc::new(SerialManager::new());

    // Print packets as they arrive
    let on_packet = Arc::new(|pkt: IncomingPacket| {
        let hop_note = if pkt.hops > 0 {
            format!("  hops={}", pkt.hops)
        } else {
            String::new()
        };
        println!(
            "\n📻 PACKET  {} → {}  cmd=0x{:02X}  rssi={}{}",
            pkt.source.to_display_string(),
            pkt.destination.to_display_string(),
            pkt.command,
            pkt.rssi,
            hop_note,
        );
        if let Some(decoded) = pkt.decoded {
            println!("   {}", decoded);
        }
        print!("\n> ");
    });

    manager.connect(port, on_packet).await
        .with_context(|| format!("Failed to open serial port {}", port))?;

    let addr = manager.refresh_node_address(1500).await;
    println!("✅ Connected! Node address: {}\n", addr.to_display_string());
    println!("Commands: ping | wallet | wallet-mnemonic | balance <addr> | send <addr> <amount> [memo] | stats | quit");
    println!("{}", "─".repeat(60));

    // Simple line-based REPL
    loop {
        print!("> ");
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).is_err() || line.trim().is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        match parts.as_slice() {
            ["quit"] | ["exit"] | ["q"] => {
                println!("👋 Disconnecting. Much goodbye. Wow.");
                break;
            }
            ["ping"] => {
                let ok = manager.ping().await;
                println!("{}", if ok { "✅ PONG!" } else { "❌ No response" });
            }
            ["wallet"] => {
                match wallet::generate_keypair() {
                    Ok(w) => {
                        println!("🆕 New wallet generated:");
                        println!("   Address:     {}", w.address);
                        println!("   Private key: {}", w.private_key_wif);
                        println!("   ⚠️  Save the private key now — not stored anywhere!");
                    }
                    Err(e) => println!("❌ Error: {}", radiodoge_core::errors::friendly(e)),
                }
            }
            ["wallet-mnemonic"] => {
                match wallet::generate_mnemonic().and_then(|m| {
                    let phrase = m.to_string();
                    wallet::wallet_from_mnemonic(&phrase).map(|w| (phrase, w))
                }) {
                    Ok((phrase, w)) => {
                        println!("🌱 New wallet with recovery phrase:");
                        for (i, word) in phrase.split_whitespace().enumerate() {
                            println!("   {:2}. {}", i + 1, word);
                        }
                        println!("   Address: {}", w.address);
                        println!("   ⚠️  Write the phrase offline — shows once!");
                    }
                    Err(e) => println!("❌ Error: {}", radiodoge_core::errors::friendly(e)),
                }
            }
            ["send", to_addr, amount_str, memo @ ..] => {
                match amount_str.parse::<f64>() {
                    Err(_) => println!("❌ Invalid amount: {}", amount_str),
                    Ok(amount) => {
                        if !wallet::is_valid_address(to_addr) {
                            println!("❌ Invalid Dogecoin address: {}", to_addr);
                        } else {
                            let memo_str = if memo.is_empty() {
                                None
                            } else {
                                Some(memo.join(" "))
                            };
                            let payload = wallet::encode_transaction_payload(
                                to_addr,
                                amount,
                                memo_str.as_deref(),
                            );
                            match payload {
                                Err(e) => println!("❌ Encode error: {}", e),
                                Ok(bytes) => {
                                    let fw = manager.ensure_firmware_version().await;
                                    let src = manager.get_node_address().await;
                                    let dst = NodeAddress::broadcast();
                                    match radio::build_tx_frames(
                                        &src, &dst, radio::CMD_DOGE_TX, &bytes, fw.as_deref(),
                                    ) {
                                        Err(e) => println!("❌ {}", e),
                                        Ok(frames) => match manager.send_frames(frames).await {
                                            Ok(()) => println!(
                                                "✅ Sent {:.8} DOGE → {} 🐕🌙", amount, to_addr
                                            ),
                                            Err(e) => println!("❌ Failed to send: {}", radiodoge_core::errors::friendly(e)),
                                        },
                                    }
                                }
                            }
                        }
                    }
                }
            }
            ["balance", addr] => {
                if !wallet::is_valid_address(addr) {
                    println!("❌ '{}' is not a valid Dogecoin address", addr);
                } else {
                    print!("🌐 Querying chain backends... ");
                    match wallet::fetch_balance(addr).await {
                        Ok(k) => println!("💰 {:.8} DOGE", k as f64 / 1e8),
                        Err(e) => println!("❌ {}", e),
                    }
                }
            }
            ["stats"] => {
                let s = manager.get_stats().await;
                let snr_str = s.snr.map(|v| format!("{:.1} dB", v)).unwrap_or_else(|| "N/A".to_string());
                println!("📊 Sent: {}  Received: {}  RSSI: {} dBm  SNR: {}",
                    s.packets_sent, s.packets_received, s.rssi, snr_str);
            }
            ["help"] | [] => {
                println!("Commands:");
                println!("  ping                         — ping the device");
                println!("  wallet                       — generate new Dogecoin keypair");
                println!("  wallet-mnemonic              — generate wallet with 12-word BIP39 phrase");
                println!("  balance <addr>               — query confirmed balance (Core RPC → BlockCypher)");
                println!("  send <addr> <amount> [memo]  — send DOGE over LoRa");
                println!("  stats                        — show radio statistics");
                println!("  quit / exit / q              — disconnect and exit");
            }
            _ => {
                println!("❓ Unknown command. Type 'help' for a list.");
            }
        }
    }

    manager.disconnect().await.ok();
    Ok(())
}

/// Query the confirmed balance for a Dogecoin address from the configured chain backends.
async fn cmd_balance(address: &str) -> Result<()> {
    if !wallet::is_valid_address(address) {
        anyhow::bail!("'{}' is not a valid Dogecoin address (must start with 'D')", address);
    }
    println!("🌐 Querying chain backends for {}...", address);
    let koinus = wallet::fetch_balance(address).await?;
    println!("💰 Balance: {:.8} DOGE  ({} koinus)", koinus as f64 / 1e8, koinus);
    Ok(())
}

/// Lightweight SPV inclusion check: report whether a txid is mined and how deep.
async fn cmd_verify_tx(txid: &str) -> Result<()> {
    println!("🔎 Verifying transaction inclusion (lightweight SPV via chain backends)...");
    let status = spv::fetch_tx_inclusion(txid).await?;
    if status.confirmed {
        println!("✅ Confirmed! {} confirmation(s).", status.confirmations);
        if let Some(h) = status.block_height {
            println!("   Block height: {}", h);
        }
        if let Some(hash) = &status.block_hash {
            println!("   Block hash:   {}", hash);
        }
        println!("   Much included. Very chain. Wow. 🐕🌙");
    } else {
        println!("⏳ Not yet confirmed — the transaction is unconfirmed or unknown to the explorer.");
    }
    Ok(())
}

/// Build, sign, and broadcast a real P2PKH Dogecoin transaction directly to the network.
async fn cmd_broadcast(wif: &str, to: &str, amount: f64, fee: Option<f64>) -> Result<()> {
    if !wallet::is_valid_address(to) {
        anyhow::bail!("'{}' is not a valid Dogecoin address (must start with 'D')", to);
    }
    if amount <= 0.0 {
        anyhow::bail!("Amount must be > 0 DOGE");
    }

    println!("🐕 Building signed P2PKH transaction...");
    println!("   To:     {}", to);
    let (raw_tx, paid) = wallet::build_signed_transaction_with_fee(wif, to, amount, fee)
        .await
        .context("Failed to build/sign transaction")?;
    println!("   Amount: {:.8} DOGE  (+{:.8} DOGE fee)", amount, paid);

    let raw_hex = hex::encode(&raw_tx);
    println!("   Signed: {} bytes", raw_tx.len());

    println!("🌐 Broadcasting via the configured chain backends (Core RPC, then BlockCypher)...");
    let txid = wallet::broadcast_raw_tx(&raw_hex)
        .await
        .context("Broadcast failed")?;

    println!("✅ Broadcast! txid = {}", txid);
    println!("   Track: https://dogechain.info/tx/{}", txid);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A duplicate broadcast is the normal outcome of a transaction addressed to
    /// the broadcast address — every board in range hands it to its host — so it
    /// has to be recognised as success. Treating it as failure cost the sender
    /// its acknowledgement even though the transaction was on the network.
    #[test]
    fn duplicate_broadcast_errors_are_recognised() {
        for msg in [
            "Network rejected transaction: transaction already in block chain",
            "Network rejected transaction: txn-already-in-mempool",
            "Network rejected transaction: txn-already-known",
            "-27: Transaction already in block chain",
            "Duplicate transaction",
            "TX ALREADY IN THE MEMPOOL",
        ] {
            assert!(
                is_duplicate_broadcast_error(msg),
                "should be treated as already-broadcast: {}",
                msg
            );
        }

        // Real failures must still retry and, eventually, report failure.
        for msg in [
            "Network rejected transaction: bad-txns-inputs-missingorspent",
            "Broadcast request failed: connection timed out",
            "Network rejected transaction: dust",
            "insufficient fee",
        ] {
            assert!(
                !is_duplicate_broadcast_error(msg),
                "should NOT be treated as already-broadcast: {}",
                msg
            );
        }
    }
}
