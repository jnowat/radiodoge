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
use radiodoge_core::types::{IncomingPacket, NodeAddress};

// ─── CLI Definition ──────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(
    name = "radiodoge-cli",
    version,
    about = "🐕 RadioDoge CLI — Wireless P2P Dogecoin over LoRa\n\nMuch CLI. Very terminal. Such Rust. Wow.",
    long_about = None,
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Enable verbose debug logging
    #[arg(short, long, global = true)]
    verbose: bool,

    /// Chain backends to try in order (default: core,blockcypher).
    /// Items: core[=URL], blockcypher[=URL], blockbook=URL. Env: RADIODOGE_BACKENDS
    #[arg(long, global = true, value_name = "LIST")]
    backends: Option<String>,

    /// Dogecoin Core JSON-RPC URL (default http://127.0.0.1:22555). Env: RADIODOGE_RPC_URL
    #[arg(long, global = true, value_name = "URL")]
    rpc_url: Option<String>,

    /// Dogecoin Core rpcuser; the password is read from RADIODOGE_RPC_PASSWORD
    /// (never pass secrets on the command line). Env: RADIODOGE_RPC_USER
    #[arg(long, global = true, value_name = "USER")]
    rpc_user: Option<String>,

    /// Path to Dogecoin Core's .cookie file (default: the platform datadir). Env: RADIODOGE_RPC_COOKIE
    #[arg(long, global = true, value_name = "PATH")]
    rpc_cookie: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// List all available serial ports on this system
    ///
    /// Example: radiodoge-cli ports
    Ports,

    /// Wallet operations (generate, validate)
    Wallet {
        #[command(subcommand)]
        cmd: WalletCommands,
    },

    /// Send a Dogecoin transaction over LoRa radio
    ///
    /// Without --wif: sends a legacy payload (amount + address) for gateways
    /// that handle signing server-side.
    /// With --wif: builds and signs a real P2PKH transaction (fetches UTXOs
    /// from chain backends, signs with secp256k1) before broadcasting over LoRa.
    ///
    /// Example: radiodoge-cli send -p COM3 -t DH5yaieq... -a 4.20 --wif Q...
    Send {
        /// Serial port (e.g. COM3 on Windows, /dev/ttyUSB0 on Linux)
        #[arg(short, long)]
        port: String,

        /// Recipient Dogecoin address (must start with 'D')
        #[arg(short = 't', long = "to")]
        to_address: String,

        /// Amount in DOGE (e.g. 4.20)
        #[arg(short, long)]
        amount: f64,

        /// Optional transaction memo (up to 190 bytes; ignored when --wif is set)
        #[arg(short, long)]
        memo: Option<String>,

        /// WIF private key for signing a real P2PKH transaction before LoRa broadcast
        #[arg(short, long)]
        wif: Option<String>,
    },

    /// Listen for incoming LoRa packets and print them
    ///
    /// Example: radiodoge-cli receive -p COM3 --timeout 60
    Receive {
        /// Serial port
        #[arg(short, long)]
        port: String,

        /// Stop after this many seconds (0 = run forever)
        #[arg(short = 'T', long, default_value = "30")]
        timeout: u64,
    },

    /// Ping the Heltec device and report round-trip success/failure
    ///
    /// Example: radiodoge-cli ping -p COM3
    Ping {
        /// Serial port
        #[arg(short, long)]
        port: String,
    },

    /// Interactive REPL — replaces RadioDogeSharp's menu-driven interface
    ///
    /// Type 'help' at the prompt for available commands.
    ///
    /// Example: radiodoge-cli connect COM3
    Connect {
        /// Serial port
        port: String,
    },

    /// Run as a headless daemon — replaces serdog (C serial daemon)
    ///
    /// Connects to the device, logs all packets to stdout, and relays them
    /// indefinitely. Useful for Raspberry Pi gateway deployments.
    ///
    /// Example: radiodoge-cli daemon -p /dev/ttyUSB0
    Daemon {
        /// Serial port
        #[arg(short, long)]
        port: String,
        /// Write a JSON snapshot of the gateway's counters here every few
        /// seconds (the desktop app's gateway dashboard reads it)
        #[arg(long)]
        stats_file: Option<std::path::PathBuf>,
        /// Store-and-forward: keep transactions that could not be broadcast
        /// (internet down) in this JSON file and retry them, surviving restarts.
        /// Without it the queue is kept in memory only.
        #[arg(long)]
        outbox_file: Option<std::path::PathBuf>,
    },

    /// Query the confirmed Dogecoin balance for an address via the configured chain backends (Core RPC, then BlockCypher)
    ///
    /// Requires an internet connection.
    ///
    /// Example: radiodoge-cli balance -a DH5yaieqoZN36fDVciNyRueRGvGLR3mr7L
    Balance {
        /// Dogecoin address to query
        #[arg(short, long)]
        address: String,
    },

    /// Verify a transaction's inclusion in the Dogecoin chain (lightweight SPV)
    ///
    /// Performs a lightweight, no-full-node inclusion check via the configured chain backends (Core RPC, then BlockCypher):
    /// reports whether the txid is mined, its confirmation depth, and the block
    /// it landed in. Requires an internet connection.
    ///
    /// Example: radiodoge-cli verify-tx 5b2a3f53f605d62c53e62932dac6925e3d74afa5a4b459745c36d42d0ed26a69
    VerifyTx {
        /// Transaction id (64 hex characters)
        #[arg(short = 't', long = "txid")]
        txid: String,
    },

    /// Build, sign, and broadcast a real P2PKH Dogecoin transaction to the network
    ///
    /// Fetches UTXOs from the configured chain backends, builds the transaction, signs each
    /// input with SIGHASH_ALL (secp256k1), and broadcasts via the configured backends.
    /// Requires an internet connection. No LoRa device needed.
    ///
    /// Example: radiodoge-cli broadcast --wif QWif... --to DH5yaie... --amount 4.20
    Broadcast {
        /// WIF-encoded private key of the sender (starts with 'Q')
        #[arg(short, long)]
        wif: String,

        /// Recipient Dogecoin address (must start with 'D')
        #[arg(short = 't', long = "to")]
        to_address: String,

        /// Amount to send in DOGE (network fee of 1 DOGE is added automatically)
        #[arg(short, long)]
        amount: f64,
    },
}

#[derive(Subcommand)]
enum WalletCommands {
    /// Generate a new Dogecoin keypair (address + private key)
    ///
    /// ⚠️  Save the private key immediately — it is NOT stored anywhere!
    ///
    /// Example: radiodoge-cli wallet generate
    Generate,

    /// Generate a new wallet with a 12-word BIP39 mnemonic recovery phrase
    ///
    /// Derives the key at m/44'/3'/0'/0/0 (Dogecoin BIP44 path).
    /// Write down the phrase offline — it is shown once and never stored.
    ///
    /// Example: radiodoge-cli wallet mnemonic
    Mnemonic,

    /// Import a wallet from a BIP39 mnemonic recovery phrase
    ///
    /// Derives the key at m/44'/3'/0'/0/0 (Dogecoin BIP44 coin type 3).
    ///
    /// Example: radiodoge-cli wallet import-mnemonic "word1 word2 ... word12"
    ImportMnemonic {
        /// 12 or 24-word BIP39 mnemonic phrase (space-separated, quoted)
        phrase: String,
    },

    /// Import a wallet from a WIF-encoded private key
    ///
    /// Only compressed Dogecoin mainnet keys (starting with 'Q') are supported.
    ///
    /// Example: radiodoge-cli wallet import-wif QWif...
    ImportWif {
        /// WIF-encoded private key (starts with 'Q' for Dogecoin mainnet)
        wif: String,
    },

    /// Validate whether a string is a valid Dogecoin address
    ///
    /// Example: radiodoge-cli wallet validate DH5yaieqoZN36fDVciNyRueRGvGLR3mr7L
    Validate {
        /// Dogecoin address to validate
        address: String,
    },
}

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

    match cli.command {
        Commands::Ports => cmd_ports(),
        Commands::Wallet { cmd } => cmd_wallet(cmd),
        Commands::Send { port, to_address, amount, memo, wif } => {
            cmd_send(&port, &to_address, amount, memo.as_deref(), wif.as_deref()).await
        }
        Commands::Receive { port, timeout } => cmd_receive(&port, timeout).await,
        Commands::Ping { port } => cmd_ping(&port).await,
        Commands::Connect { port } => cmd_connect(&port).await,
        Commands::Daemon { port, stats_file, outbox_file } => cmd_daemon(&port, stats_file, outbox_file).await,
        Commands::Balance { address } => cmd_balance(&address).await,
        Commands::VerifyTx { txid } => cmd_verify_tx(&txid).await,
        Commands::Broadcast { wif, to_address, amount } => cmd_broadcast(&wif, &to_address, amount).await,
    }
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
async fn cmd_send(port: &str, to: &str, amount: f64, memo: Option<&str>, wif: Option<&str>) -> Result<()> {
    if !wallet::is_valid_address(to) {
        anyhow::bail!("'{}' is not a valid Dogecoin address (must start with 'D')", to);
    }
    if amount <= 0.0 {
        anyhow::bail!("Amount must be > 0 DOGE");
    }

    println!("🐕 Connecting to {} ...", port);
    let manager = Arc::new(SerialManager::new());
    let on_packet = Arc::new(|_pkt: IncomingPacket| {});
    manager.connect(port, on_packet).await
        .with_context(|| format!("Failed to open serial port {}", port))?;

    // Build payload: real signed P2PKH tx when --wif supplied, legacy stub otherwise.
    let payload = if let Some(key) = wif {
        println!("🔑 Signing P2PKH transaction (fetching UTXOs from chain backends)...");
        wallet::build_signed_transaction(key, to, amount, wallet::DEFAULT_TX_FEE_DOGE)
            .await
            .context("Transaction signing failed")?
    } else {
        println!("✅ Connected! Encoding stub transaction...");
        wallet::encode_transaction_payload(to, amount, memo)
            .context("Failed to encode transaction")?
    };

    // Which framing the board can accept depends on its firmware, so ask before
    // deciding. `build_tx_frames` refuses anything the board would mis-frame
    // rather than transmitting a transaction no receiver can reconstruct.
    let firmware = manager.ensure_firmware_version().await;
    let src = manager.get_node_address().await;
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

    let addr = manager.get_node_address().await;
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
                    Err(e) => println!("❌ Error: {}", e),
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
                    Err(e) => println!("❌ Error: {}", e),
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
                                            Err(e) => println!("❌ Failed to send: {}", e),
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

/// Headless daemon — replaces serdog (C serial daemon).
///
/// Connects to the device, logs all received packets indefinitely.
/// When a signed Dogecoin transaction is received (CMD_DOGE_TX with a raw
/// transaction payload), broadcasts it to the Dogecoin network via the
/// configured chain backends and sends a TX_ACK message back to the originator.
///
/// Multipart sequences are reassembled by the serial read loop before this
/// callback runs, so a transaction that arrived split across several frames is
/// delivered here as one complete payload — no reassembly needed at this layer.
///
/// Designed for Raspberry Pi gateway deployments.
/// Shared, lock-protected gateway counters. A std mutex: every critical section
/// is a few field updates and never awaits.
type SharedStats = Arc<std::sync::Mutex<GatewayStats>>;
type SharedOutbox = Arc<std::sync::Mutex<Outbox>>;
/// Transactions already broadcast this run: local txid → network txid.
type SeenTxs = Arc<tokio::sync::Mutex<std::collections::HashMap<String, String>>>;

/// Write `contents` to `path` through a temporary file so a reader never sees half a file.
fn write_atomic(path: &std::path::Path, contents: &str) {
    let tmp = path.with_extension("json.tmp");
    if let Err(e) = std::fs::write(&tmp, contents).and_then(|_| std::fs::rename(&tmp, path)) {
        log::warn!("Could not write {}: {}", path.display(), e);
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn stats_record(stats: &SharedStats, kind: GatewayEventKind, node: &NodeAddress, detail: impl Into<String>) {
    if let Ok(mut s) = stats.lock() {
        s.record(kind, &node.to_display_string(), detail, unix_now());
    }
}

/// Write the snapshot next to its destination and rename it into place, so a
/// reader never sees half a file.
fn write_stats_file(path: &std::path::Path, stats: &SharedStats) {
    let json = match stats.lock() {
        Ok(mut s) => {
            s.updated_at = unix_now();
            serde_json::to_vec_pretty(&*s)
        }
        Err(_) => return,
    };
    let Ok(json) = json else { return };
    let tmp = path.with_extension("json.tmp");
    if let Err(e) = std::fs::write(&tmp, json).and_then(|_| std::fs::rename(&tmp, path)) {
        log::warn!("Could not write gateway stats to {}: {}", path.display(), e);
    }
}

async fn cmd_daemon(
    port: &str,
    stats_file: Option<std::path::PathBuf>,
    outbox_file: Option<std::path::PathBuf>,
) -> Result<()> {
    println!("🐕 RadioDoge Daemon — Gateway Mode (replaces serdog)");
    println!("Serial port: {}", port);
    println!("Press Ctrl-C to stop.\n");

    // NOTE: logging is already initialised in main() — calling
    // env_logger::Builder::init() a second time here panicked at daemon startup.

    let manager = Arc::new(SerialManager::new());
    let manager_for_ack = Arc::clone(&manager);
    let stats: SharedStats = Arc::new(std::sync::Mutex::new(GatewayStats::new(port, unix_now())));
    let stats_for_packets = Arc::clone(&stats);

    // Store-and-forward outbox (see radiodoge_core::store_forward).
    let initial = outbox_file
        .as_deref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| Outbox::from_json(&s))
        .unwrap_or_default();
    if !initial.is_empty() {
        log::info!(
            "Outbox: {} transaction(s) and {} reply(ies) carried over from the last run",
            initial.broadcasts.len(),
            initial.replies.len()
        );
    }
    let outbox: SharedOutbox = Arc::new(std::sync::Mutex::new(initial));
    let outbox_for_packets = Arc::clone(&outbox);

    // Transactions already handled this session, keyed by txid.
    //
    // A transaction addressed to the broadcast address is forwarded to its host
    // by *every* board that hears it, and the sender retransmits an
    // unacknowledged multipart part, so the same signed bytes reach a gateway
    // more than once as a matter of course. Re-POSTing them is harmless to the
    // network — the txid is identical — but the second attempt is rejected as a
    // duplicate, which used to look like a broadcast failure and cost the sender
    // its acknowledgement. Remembering what has been broadcast turns a repeat
    // into an immediate re-ACK instead.
    let seen_txs: SeenTxs =
        Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()));
    let seen_for_outbox = Arc::clone(&seen_txs);

    let on_packet = Arc::new(move |pkt: IncomingPacket| {
        log::info!(
            "PACKET  {} → {}  cmd=0x{:02X}  rssi={}{}  payload={}{}",
            pkt.source.to_display_string(),
            pkt.destination.to_display_string(),
            pkt.command,
            pkt.rssi,
            if pkt.hops > 0 { format!("  hops={}", pkt.hops) } else { String::new() },
            pkt.payload_hex,
            pkt.decoded
                .as_deref()
                .map(|d| format!("  decoded={}", d))
                .unwrap_or_default(),
        );
        if let Ok(mut s) = stats_for_packets.lock() {
            s.record_packet(&pkt.source.to_display_string(), pkt.rssi, pkt.hops, unix_now());
        }

        // Store-and-forward: the node is in range right now, so release any
        // replies that could not be sent to it earlier.
        let held = outbox_for_packets
            .lock()
            .map(|mut o| o.take_replies_for(&pkt.source.to_display_string()))
            .unwrap_or_default();
        if !held.is_empty() {
            let mgr = Arc::clone(&manager_for_ack);
            let source = pkt.source.clone();
            let st = Arc::clone(&stats_for_packets);
            let ob = Arc::clone(&outbox_for_packets);
            tokio::spawn(async move {
                for r in held {
                    send_reply(&r.text, &mgr, &source, &st, &ob, GatewayEventKind::ReplyForwarded).await;
                }
            });
        }

        // When a signed Dogecoin transaction arrives, broadcast it to the network.
        if pkt.command == radio::CMD_DOGE_TX {
            let payload_bytes = hex::decode(&pkt.payload_hex).unwrap_or_default();
            if wallet::is_signed_tx_payload(&payload_bytes) {
                let raw_hex = pkt.payload_hex.clone();
                let txid = wallet::compute_txid(&payload_bytes);
                let mgr = Arc::clone(&manager_for_ack);
                let seen = Arc::clone(&seen_txs);
                let source = pkt.source.clone();
                let st = Arc::clone(&stats_for_packets);
                let ob = Arc::clone(&outbox_for_packets);
                stats_record(&st, GatewayEventKind::TxReceived, &source, txid.clone());
                log::info!(
                    "GATEWAY  signed tx detected ({} bytes, txid {}) from {} — broadcasting to Dogecoin network",
                    payload_bytes.len(),
                    txid,
                    source.to_display_string()
                );
                tokio::spawn(async move {
                    daemon_broadcast_and_ack(raw_hex, txid, mgr, source, seen, st, ob).await;
                });
            }
        }

        // When a balance request arrives, query the chain backends and send the result back.
        if pkt.command == radio::CMD_REQUEST_BALANCE {
            let payload_bytes = hex::decode(&pkt.payload_hex).unwrap_or_default();
            let addr = String::from_utf8_lossy(&payload_bytes)
                .trim_matches('\0')
                .trim()
                .to_string();
            if !addr.is_empty() {
                let mgr = Arc::clone(&manager_for_ack);
                let source = pkt.source.clone();
                let st = Arc::clone(&stats_for_packets);
                let ob = Arc::clone(&outbox_for_packets);
                stats_record(&st, GatewayEventKind::BalanceRequest, &source, "");
                log::info!("GATEWAY  balance request from {} for {}", source.to_display_string(), addr);
                tokio::spawn(async move {
                    daemon_fetch_and_send_balance(addr, mgr, source, st, ob).await;
                });
            }
        }
    });

    log::info!("Connecting to {} ...", port);
    manager.connect(port, on_packet).await
        .with_context(|| format!("Failed to open serial port {}", port))?;

    let addr = manager.get_node_address().await;
    log::info!("Connected — node address: {}", addr.to_display_string());
    if let Ok(mut s) = stats.lock() {
        s.gateway_address = Some(addr.to_display_string());
    }

    if let Some(fw) = manager.ensure_firmware_version().await {
        log::info!("Board firmware: {}", fw);
        if let Ok(mut s) = stats.lock() {
            s.firmware = Some(fw.clone());
        }
        if !radio::firmware_supports_multipart(Some(&fw)) {
            log::warn!(
                "This board predates firmware v0.4.2 (FW{}). It cannot relay a transaction \
                 larger than one {}-byte packet, so senders will be limited to the smallest \
                 possible transactions. Flash heltec-firmware-v3/ to lift the limit.",
                radio::MIN_MULTIPART_FIRMWARE,
                radio::MAX_SINGLE_PAYLOAD_LEN
            );
        }
    }

    // Put the board into gateway mode. The board only relays a host MESSAGE over
    // LoRa when this is set, and TX_ACK is a host MESSAGE — so without it a
    // gateway broadcasts transactions perfectly and the sender never hears back.
    // Asking for it here means running the daemon is enough; there is no
    // separate step in the GUI to remember.
    if let Err(e) = manager.send_raw(radio::build_set_gateway(&addr, true)).await {
        log::warn!("Could not enable gateway mode on the board: {}", e);
    } else {
        log::info!("Gateway mode enabled on the board (needed to radio TX_ACK back to senders)");
    }

    log::info!("Gateway ready — monitoring for signed Dogecoin transactions");

    if let Some(path) = stats_file.clone() {
        log::info!("Writing gateway stats to {}", path.display());
        let st = Arc::clone(&stats);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(3));
            loop {
                tick.tick().await;
                write_stats_file(&path, &st);
            }
        });
    }

    // Store-and-forward worker: retry queued broadcasts, expire stale work,
    // keep the dashboard gauges and the outbox file current.
    {
        let ob = Arc::clone(&outbox);
        let st = Arc::clone(&stats);
        let mgr = Arc::clone(&manager);
        let seen = seen_for_outbox;
        let path = outbox_file.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(10));
            let mut last_saved = String::new();
            loop {
                tick.tick().await;
                outbox_tick(&ob, &st, &mgr, &seen).await;
                if let Some(p) = path.as_deref() {
                    let json = ob.lock().map(|o| o.to_json()).unwrap_or_default();
                    if json != last_saved {
                        write_atomic(p, &json);
                        last_saved = json;
                    }
                }
            }
        });
    }

    // Run until Ctrl-C
    tokio::signal::ctrl_c().await.context("Failed to listen for Ctrl-C")?;
    if let Some(p) = outbox_file.as_deref() {
        if let Ok(o) = outbox.lock() {
            write_atomic(p, &o.to_json());
        }
    }
    if let Some(path) = stats_file.as_deref() {
        write_stats_file(path, &stats);
    }

    log::info!("Shutdown signal received — disconnecting");
    manager.disconnect().await.ok();

    Ok(())
}

/// `true` if a broadcast error means the network already has this transaction.
///
/// Every node worth talking to rejects a duplicate, and each phrases it
/// differently. A duplicate is a *success* from the sender's point of view — the
/// transaction is on the network — so it must produce an acknowledgement rather
/// than a retry loop that ends in "broadcast failed".
fn is_duplicate_broadcast_error(err: &str) -> bool {
    let e = err.to_ascii_lowercase();
    e.contains("already in block chain")
        || e.contains("already in the mempool")
        || e.contains("already in mempool")
        || e.contains("already known")
        || e.contains("txn-already-known")
        || e.contains("txn-already-in-mempool")
        || e.contains("transaction already exists")
        || e.contains("already exists")
        || e.contains("duplicate transaction")
}

/// Broadcast a signed transaction to the Dogecoin network with exponential-backoff
/// retry (up to 3 attempts), then radio an ACK back to the originating node.
///
/// `txid` is computed locally from the raw bytes, so the acknowledgement carries
/// the right id even when the network answers "already known" instead of
/// returning one.
async fn daemon_broadcast_and_ack(
    raw_hex: String,
    txid: String,
    mgr: Arc<SerialManager>,
    source: NodeAddress,
    seen: SeenTxs,
    stats: SharedStats,
    outbox: SharedOutbox,
) {
    // A transaction this gateway already put on the network only needs its
    // acknowledgement re-sent — the sender may simply not have heard the first.
    if let Some(known) = seen.lock().await.get(&txid).cloned() {
        log::info!("GATEWAY  txid={} already broadcast this session — re-sending ACK", known);
        if let Ok(mut s) = stats.lock() {
            s.record_duplicate(unix_now());
        }
        daemon_send_tx_ack(&known, &mgr, &source, &stats, &outbox).await;
        return;
    }

    let mut delay_secs = 2u64;
    for attempt in 0..3u32 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_secs(delay_secs)).await;
            delay_secs *= 2;
        }
        match wallet::broadcast_raw_tx(&raw_hex).await {
            Ok(network_txid) => {
                if network_txid != txid {
                    // Not fatal — the network's answer wins — but it means the
                    // bytes were interpreted differently than expected, which is
                    // worth seeing in a gateway log.
                    log::warn!(
                        "GATEWAY  network returned txid={} but the payload hashes to {}",
                        network_txid, txid
                    );
                }
                log::info!("GATEWAY  broadcast OK  txid={}", network_txid);
                seen.lock().await.insert(txid.clone(), network_txid.clone());
                stats_record(&stats, GatewayEventKind::TxBroadcast, &source, network_txid.clone());
                daemon_send_tx_ack(&network_txid, &mgr, &source, &stats, &outbox).await;
                return;
            }
            Err(e) => {
                let msg = e.to_string();
                if is_duplicate_broadcast_error(&msg) {
                    log::info!(
                        "GATEWAY  network already has txid={} — acknowledging as broadcast",
                        txid
                    );
                    seen.lock().await.insert(txid.clone(), txid.clone());
                    stats_record(&stats, GatewayEventKind::TxBroadcast, &source, format!("{} (already on network)", txid));
                    daemon_send_tx_ack(&txid, &mgr, &source, &stats, &outbox).await;
                    return;
                }
                log::warn!("GATEWAY  broadcast attempt {}/3 failed: {}", attempt + 1, msg);
            }
        }
    }
    // Store-and-forward: keep it and keep trying. The usual cause is a gateway
    // that has lost its internet link, and the sender has no other way to reach
    // the network.
    log::warn!("GATEWAY  broadcast failed after 3 attempts for txid={} — queued for retry", txid);
    let queued = outbox
        .lock()
        .map(|mut o| o.queue_broadcast(&txid, &raw_hex, &source.to_display_string(), unix_now()))
        .unwrap_or(false);
    if queued {
        stats_record(&stats, GatewayEventKind::TxQueued, &source, txid);
    }
}

/// One pass of the store-and-forward worker.
async fn outbox_tick(outbox: &SharedOutbox, stats: &SharedStats, mgr: &Arc<SerialManager>, seen: &SeenTxs) {
    let now = unix_now();
    let (due, expired) = match outbox.lock() {
        Ok(mut o) => {
            let before: Vec<PendingBroadcast> = o.broadcasts.clone();
            let gone = o.expire(now);
            let expired: Vec<PendingBroadcast> =
                before.into_iter().filter(|b| !o.is_queued(&b.txid)).collect();
            if gone.1 > 0 {
                log::warn!("GATEWAY  {} held reply(ies) expired before their node was heard again", gone.1);
            }
            (o.due_broadcasts(now), expired)
        }
        Err(_) => return,
    };
    for b in expired {
        let node = parse_node(&b.source).unwrap_or_default();
        log::error!("GATEWAY  txid={} expired in the outbox after {} retries", b.txid, b.attempts);
        stats_record(stats, GatewayEventKind::TxFailed, &node, b.txid.clone());
        stats_record(stats, GatewayEventKind::Expired, &node, b.txid);
    }
    for b in due {
        let node = parse_node(&b.source).unwrap_or_default();
        let accepted = match wallet::broadcast_raw_tx(&b.raw_hex).await {
            Ok(id) => Some(id),
            Err(e) if is_duplicate_broadcast_error(&e.to_string()) => Some(b.txid.clone()),
            Err(e) => {
                log::info!("GATEWAY  outbox retry {} for txid={} failed: {}", b.attempts + 1, b.txid, e);
                None
            }
        };
        match accepted {
            Some(id) => {
                log::info!("GATEWAY  outbox: txid={} broadcast on retry {}", id, b.attempts + 1);
                if let Ok(mut o) = outbox.lock() {
                    o.broadcast_done(&b.txid);
                }
                seen.lock().await.insert(b.txid.clone(), id.clone());
                stats_record(stats, GatewayEventKind::TxBroadcast, &node, id.clone());
                daemon_send_tx_ack(&id, mgr, &node, stats, outbox).await;
            }
            None => {
                if let Ok(mut o) = outbox.lock() {
                    o.broadcast_failed(&b.txid, unix_now());
                }
            }
        }
    }
    if let (Ok(o), Ok(mut s)) = (outbox.lock(), stats.lock()) {
        s.outbox_broadcasts = o.broadcasts.len() as u64;
        s.outbox_replies = o.replies.len() as u64;
    }
}

/// Radio `text` to `dest`; if the board will not take it, hold it until `dest` is heard again.
async fn send_reply(
    text: &str,
    mgr: &Arc<SerialManager>,
    dest: &NodeAddress,
    stats: &SharedStats,
    outbox: &SharedOutbox,
    ok_kind: GatewayEventKind,
) -> bool {
    let gateway_addr = mgr.get_node_address().await;
    let pkt = radio::build_message(&gateway_addr, dest, text);
    match mgr.send_raw(pkt).await {
        Ok(()) => {
            stats_record(stats, ok_kind, dest, reply_detail(text));
            true
        }
        Err(e) => {
            log::warn!("GATEWAY  reply to {} not sent ({}) — held until the node is heard again", dest.to_display_string(), e);
            if let Ok(mut o) = outbox.lock() {
                o.queue_reply(&dest.to_display_string(), text, unix_now());
            }
            stats_record(stats, GatewayEventKind::ReplyQueued, dest, e.to_string());
            false
        }
    }
}

/// What the dashboard shows for a reply: the txid for an ACK, the amount for a balance.
fn reply_detail(text: &str) -> String {
    if let Some(id) = text.strip_prefix("TX_ACK:") {
        id.to_string()
    } else if let Some(k) = text.strip_prefix("BAL:").and_then(|k| k.parse::<u64>().ok()) {
        format!("{:.8} DOGE", k as f64 / 1e8)
    } else {
        String::new()
    }
}

/// Radio a `TX_ACK:<txid>` back to the node that sent the transaction.
///
/// The full 64-hex-character txid is included — `"TX_ACK:"` + txid is 71 bytes,
/// well inside a single packet — so the sender can actually look it up and
/// verify inclusion.
async fn daemon_send_tx_ack(
    txid: &str,
    mgr: &Arc<SerialManager>,
    source: &NodeAddress,
    stats: &SharedStats,
    outbox: &SharedOutbox,
) {
    if !send_reply(&format!("TX_ACK:{}", txid), mgr, source, stats, outbox, GatewayEventKind::AckSent).await {
        stats_record(stats, GatewayEventKind::AckFailed, source, "held for retry");
    }
}

/// Fetch the balance for `address` from the chain backends and send it back to `source` via radio.
/// The response is a CMD_MESSAGE with text `"BAL:{koinus}"` so the GUI can parse it.
async fn daemon_fetch_and_send_balance(
    address: String,
    mgr: Arc<SerialManager>,
    source: NodeAddress,
    stats: SharedStats,
    outbox: SharedOutbox,
) {
    match wallet::fetch_balance(&address).await {
        Ok(koinus) => {
            log::info!(
                "GATEWAY  balance for {}: {} koinus ({:.8} DOGE)",
                address, koinus, koinus as f64 / 1e8
            );
            // The dashboard shows the amount, never the address asked about.
            if !send_reply(&format!("BAL:{}", koinus), &mgr, &source, &stats, &outbox, GatewayEventKind::BalanceReplied).await {
                stats_record(&stats, GatewayEventKind::BalanceFailed, &source, "reply held for retry");
            }
        }
        Err(e) => {
            log::warn!("GATEWAY  balance fetch failed for {}: {}", address, e);
            stats_record(&stats, GatewayEventKind::BalanceFailed, &source, e.to_string());
        }
    }
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
async fn cmd_broadcast(wif: &str, to: &str, amount: f64) -> Result<()> {
    if !wallet::is_valid_address(to) {
        anyhow::bail!("'{}' is not a valid Dogecoin address (must start with 'D')", to);
    }
    if amount <= 0.0 {
        anyhow::bail!("Amount must be > 0 DOGE");
    }

    println!("🐕 Building signed P2PKH transaction...");
    println!("   To:     {}", to);
    println!("   Amount: {:.8} DOGE  (+{:.8} DOGE fee)", amount, wallet::DEFAULT_TX_FEE_DOGE);

    let raw_tx = wallet::build_signed_transaction(wif, to, amount, wallet::DEFAULT_TX_FEE_DOGE)
        .await
        .context("Failed to build/sign transaction")?;

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
