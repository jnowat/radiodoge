//! Command-line definition: the clap `Cli` parser and its subcommand enums.

use super::*;

#[derive(Parser)]
#[command(
    name = "radiodoge-cli",
    version,
    about = "🐕 RadioDoge CLI — Wireless P2P Dogecoin over LoRa\n\nMuch CLI. Very terminal. Such Rust. Wow.",
    long_about = None,
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Commands,

    /// Enable verbose debug logging
    #[arg(short, long, global = true)]
    pub(crate) verbose: bool,

    /// Chain backends to try in order (default: core,blockcypher).
    /// Items: core[=URL], blockcypher[=URL], blockbook=URL. Env: RADIODOGE_BACKENDS
    #[arg(long, global = true, value_name = "LIST")]
    pub(crate) backends: Option<String>,

    /// Dogecoin Core JSON-RPC URL (default http://127.0.0.1:22555). Env: RADIODOGE_RPC_URL
    #[arg(long, global = true, value_name = "URL")]
    pub(crate) rpc_url: Option<String>,

    /// Dogecoin Core rpcuser; the password is read from RADIODOGE_RPC_PASSWORD
    /// (never pass secrets on the command line). Env: RADIODOGE_RPC_USER
    #[arg(long, global = true, value_name = "USER")]
    pub(crate) rpc_user: Option<String>,

    /// Path to Dogecoin Core's .cookie file (default: the platform datadir). Env: RADIODOGE_RPC_COOKIE
    #[arg(long, global = true, value_name = "PATH")]
    pub(crate) rpc_cookie: Option<String>,
}

#[derive(Subcommand)]
pub(crate) enum Commands {
    /// List all available serial ports on this system
    ///
    /// Example: radiodoge-cli ports
    Ports,

    /// Meshtastic frame tools (offline): wrap RadioDoge packets / decode frames
    Meshtastic {
        #[command(subcommand)]
        cmd: MeshtasticCommands,
    },

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

        /// Network fee in DOGE when signing with --wif (default: current network rate from the chain backends, never below 0.01 DOGE per started kB)
        #[arg(long)]
        fee: Option<f64>,

        /// With --wif: gateway node to address (default: broadcast to any gateway)
        #[arg(short, long)]
        gateway: Option<String>,

        /// With --wif: seconds to wait for the gateway's TX_ACK
        #[arg(short = 'T', long, default_value = "90")]
        timeout: u64,

        /// With --wif: radio even if this txid was sent in the last 30 minutes
        #[arg(long)]
        force: bool,
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

    /// Set the board's LoRa node address (persisted in NVS) over USB serial
    ///
    /// Example: radiodoge-cli set-address -p COM3 10.0.2
    SetAddress {
        #[arg(short, long)]
        port: String,
        /// New address, Region.Community.Node (e.g. 10.0.2)
        address: String,
    },

    /// Send a text message over LoRa to another node (multipart above 192 bytes)
    ///
    /// Example: radiodoge-cli message -p COM3 -t 10.0.1 "much hello"
    Message {
        #[arg(short, long)]
        port: String,
        /// Destination node (Region.Community.Node, or 255.255.255 for all)
        #[arg(short, long)]
        to: String,
        /// Text to send
        text: String,
        /// Then listen this many seconds for replies (0 = don't)
        #[arg(short = 'T', long, default_value = "0")]
        listen: u64,
    },

    /// Ping another node over the air and wait for its LoRa ACK
    ///
    /// Example: radiodoge-cli lora-ping -p COM3 -t 10.0.1
    LoraPing {
        #[arg(short, long)]
        port: String,
        #[arg(short, long)]
        to: String,
        /// Seconds to wait for the ACK
        #[arg(short = 'T', long, default_value = "10")]
        timeout: u64,
    },

    /// Ask a LoRa gateway for an address's balance and wait for the BAL: reply
    ///
    /// Example: radiodoge-cli lora-balance -p COM3 -a D...
    LoraBalance {
        #[arg(short, long)]
        port: String,
        #[arg(short, long)]
        address: String,
        /// Gateway node to ask (default: broadcast to any gateway)
        #[arg(short, long)]
        gateway: Option<String>,
        #[arg(short = 'T', long, default_value = "30")]
        timeout: u64,
    },

    /// Radio an already-signed transaction (hex file) to a gateway and wait for TX_ACK
    ///
    /// Example: radiodoge-cli send-tx-hex -p COM3 -f tx.hex -g 10.0.1
    SendTxHex {
        #[arg(short, long)]
        port: String,
        /// File containing the raw signed transaction as hex
        #[arg(short, long)]
        file: String,
        /// Gateway node (default: broadcast to any gateway)
        #[arg(short, long)]
        gateway: Option<String>,
        #[arg(short = 'T', long, default_value = "90")]
        timeout: u64,
        /// Radio even if this txid was sent in the last 30 minutes
        #[arg(long)]
        force: bool,
    },

    /// Build and sign a P2PKH transaction WITHOUT broadcasting it or touching a board.
    ///
    /// The WIF is read from the RADIODOGE_WIF environment variable (never pass keys
    /// on the command line). Writes the raw hex to --out and prints txid + size.
    ///
    /// Example: RADIODOGE_WIF=Q... radiodoge-cli sign-tx -t D... -a 1 -o tx.hex
    SignTx {
        #[arg(short = 't', long = "to")]
        to_address: String,
        #[arg(short, long)]
        amount: f64,
        #[arg(short, long)]
        out: String,
        /// Fee in DOGE (default: current network rate from the chain backends, never below 0.01 DOGE per started kB)
        #[arg(long)]
        fee: Option<f64>,
    },

    /// Save an address's UTXOs (public chain data) to a JSON file for offline signing.
    ///
    /// Later, set RADIODOGE_UTXO_FILE to that file and sign-tx signs from it with
    /// no network (pass --fee so no fee estimate is fetched either).
    /// Example: radiodoge-cli utxo-snapshot -a D... -o utxos.json
    UtxoSnapshot {
        #[arg(short, long)]
        address: String,
        #[arg(short, long)]
        out: String,
    },

    /// Print the board's node address and firmware version (read-only, nothing is aired)
    ///
    /// With --expect, exits non-zero unless the address matches (used by the
    /// hardware-session harness). Example: radiodoge-cli info -p COM3 --expect 10.0.1
    Info {
        #[arg(short, long)]
        port: String,
        /// Fail unless the board reports this node address
        #[arg(long)]
        expect: Option<String>,
    },

    /// Ping the Heltec device and report round-trip success/failure
    ///
    /// Example: radiodoge-cli ping -p COM3
    Ping {
        /// Serial port
        #[arg(short, long)]
        port: String,
    },

    /// Discover nodes on the current channel (broadcast PING, list who ACKs)
    ///
    /// Example: radiodoge-cli discover -p COM3 --listen 8
    Discover {
        /// Serial port
        #[arg(short, long)]
        port: String,
        /// Seconds to listen for answers (1-30)
        #[arg(long, default_value_t = 6)]
        listen: u64,
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

        /// Amount to send in DOGE (the network fee is added on top)
        #[arg(short, long)]
        amount: f64,

        /// Network fee in DOGE (default: current network rate from the chain backends, never below 0.01 DOGE per started kB)
        #[arg(long)]
        fee: Option<f64>,
    },

    /// Show the current network fee rate, and the fee a send would pay
    Fee {
        /// Sender address; with --amount, estimate the fee for that send
        #[arg(long)]
        from: Option<String>,
        /// Amount in DOGE (requires --from)
        #[arg(short, long, requires = "from")]
        amount: Option<f64>,
    },
}

#[derive(Subcommand)]
pub(crate) enum MeshtasticCommands {
    /// Wrap a hex RadioDoge packet in a Meshtastic frame (PRIVATE_APP, broadcast)
    ///
    /// Example: radiodoge-cli meshtastic wrap 0301027769 --from 0xdeadbeef
    Wrap {
        /// RadioDoge packet bytes as hex
        packet_hex: String,
        /// Sender node number (decimal or 0x-hex)
        #[arg(long, default_value = "0x52440001")]
        from: String,
        /// Packet id (random if omitted)
        #[arg(long)]
        id: Option<u32>,
        #[command(flatten)]
        channel: ChannelArgs,
    },
    /// Decode a hex Meshtastic frame and show its header and payload
    Decode {
        frame_hex: String,
        #[command(flatten)]
        channel: ChannelArgs,
    },
    /// Bridge mode: listen to (and optionally send on) a Meshtastic channel
    /// through the board's raw LoRa mode. Needs firmware v0.4.4 (FW12).
    ///
    /// Raw mode is volatile: the board's saved RadioDoge settings are never
    /// changed, and it returns to normal receive when this exits or times out.
    ///
    /// Example: radiodoge-cli meshtastic bridge --port COM5 --listen 120
    Bridge {
        #[arg(short, long)]
        port: String,
        /// Seconds to listen for Meshtastic frames (0 = send only)
        #[arg(long, default_value_t = 60)]
        listen: u16,
        /// Send this hex RadioDoge packet wrapped as PRIVATE_APP first
        #[arg(long)]
        send_hex: Option<String>,
        /// Send this Meshtastic text message first
        #[arg(long, conflicts_with = "send_hex")]
        text: Option<String>,
        /// Sender node number (decimal or 0x-hex)
        #[arg(long, default_value = "0x52440001")]
        from: String,
        /// Frequency in kHz (default: LongFast US slot, 906875)
        #[arg(long, default_value_t = 906_875)]
        freq_khz: u32,
        /// TX power in dBm
        #[arg(long, default_value_t = 17)]
        power: i8,
        #[command(flatten)]
        channel: ChannelArgs,
    },
}

#[derive(clap::Args)]
pub(crate) struct ChannelArgs {
    /// Channel name
    #[arg(long, default_value = "LongFast")]
    pub(crate) channel: String,
    /// Channel key as hex (16/32 bytes) or a 1-byte PSK index; default 01 = public key
    #[arg(long, default_value = "01")]
    pub(crate) key: String,
}

impl ChannelArgs {
    pub(crate) fn to_channel(&self) -> Result<radiodoge_core::meshtastic::Channel> {
        Ok(radiodoge_core::meshtastic::Channel {
            name: self.channel.clone(),
            key: hex::decode(&self.key).context("--key must be hex")?,
        })
    }
}

#[derive(Subcommand)]
pub(crate) enum WalletCommands {
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
