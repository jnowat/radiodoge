//! # Chain backends — where balances, UTXOs and broadcasts come from
//!
//! RadioDoge needs three things from the Dogecoin network: an address's
//! confirmed balance, its spendable outputs (to build a transaction), and a
//! way to push a signed transaction. Since v0.4.3 these go through an ordered
//! list of backends, tried in turn until one answers:
//!
//! 1. **`core`** (default, first) — your own Dogecoin Core node over JSON-RPC.
//!    Broadcast uses `sendrawtransaction`. Balance and UTXOs use `listunspent`,
//!    which on Dogecoin Core 1.14 only sees addresses in the node's wallet, so
//!    the address must be imported once as watch-only
//!    (`dogecoin-cli importaddress <ADDR> "radiodoge" true`). If it is not, the
//!    backend reports that instead of a misleading zero, and the next backend
//!    is tried. Core 1.14 has no `scantxoutset`.
//! 2. **`blockcypher`** (default fallback) — the public BlockCypher API
//!    (`https://api.blockcypher.com/v1/doge/main`). Unauthenticated use is rate
//!    limited; set `RADIODOGE_BLOCKCYPHER_TOKEN` for a higher quota.
//! 3. **`blockbook[=URL]`** (opt-in only) — any Blockbook v2 server. Trezor's
//!    public instance started refusing API clients (HTTP 403, Cloudflare), so
//!    it is no longer used by default; point this at a Blockbook you run.
//!
//! ## Configuration (environment, or the CLI flags that set the same values)
//!
//! | Variable | Meaning | Default |
//! |----------|---------|---------|
//! | `RADIODOGE_BACKENDS` | comma list, e.g. `core,blockcypher` or `blockbook=https://bb.example/api/v2` | `core,blockcypher` |
//! | `RADIODOGE_RPC_URL` | Core JSON-RPC URL | `http://127.0.0.1:22555` |
//! | `RADIODOGE_RPC_USER` / `RADIODOGE_RPC_PASSWORD` | `rpcuser` / `rpcpassword` | — |
//! | `RADIODOGE_RPC_COOKIE` | path to Core's `.cookie` file | the platform's default datadir |
//! | `RADIODOGE_BLOCKCYPHER_URL` | BlockCypher base URL | `https://api.blockcypher.com/v1/doge/main` |
//! | `RADIODOGE_BLOCKCYPHER_TOKEN` | BlockCypher API token | — |

use anyhow::{anyhow, bail, Result};
use std::sync::RwLock;
use std::time::Duration;

pub const DEFAULT_RPC_URL: &str = "http://127.0.0.1:22555";
pub const DEFAULT_BLOCKCYPHER_URL: &str = "https://api.blockcypher.com/v1/doge/main";

/// How to authenticate to Dogecoin Core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RpcAuth {
    None,
    UserPass { user: String, password: String },
    /// Path to the `.cookie` file Core writes when `server=1` and no
    /// `rpcpassword` is set. Read on every call, because Core rotates it on restart.
    Cookie(std::path::PathBuf),
}

/// One chain data source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Backend {
    Core { url: String, auth: RpcAuth },
    BlockCypher { base: String, token: Option<String> },
    Blockbook { base: String },
}

impl Backend {
    pub fn name(&self) -> &'static str {
        match self {
            Backend::Core { .. } => "core",
            Backend::BlockCypher { .. } => "blockcypher",
            Backend::Blockbook { .. } => "blockbook",
        }
    }
}

/// An unspent output, normalised across backends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Utxo {
    pub txid: String,
    pub vout: u32,
    pub value: u64,
    pub confirmations: u32,
    /// `true` only if the backend says so. BlockCypher and Core's `listunspent`
    /// do not report it; coin selection treats those outputs as non-coinbase.
    pub coinbase: bool,
}

/// Where a transaction is on chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxStatus {
    pub confirmations: u32,
    pub block_height: Option<u64>,
    pub block_hash: Option<String>,
}

static OVERRIDE: RwLock<Option<Vec<Backend>>> = RwLock::new(None);

/// Replace the backend list for this process (CLI flags / GUI settings).
/// `None` reverts to the environment / defaults.
pub fn set_backends(backends: Option<Vec<Backend>>) {
    *OVERRIDE.write().unwrap_or_else(|e| e.into_inner()) = backends;
}

/// The backend list in effect: an explicit override, else `RADIODOGE_BACKENDS`,
/// else `core,blockcypher`.
pub fn configured_backends() -> Result<Vec<Backend>> {
    if let Some(b) = OVERRIDE.read().unwrap_or_else(|e| e.into_inner()).clone() {
        return Ok(b);
    }
    let spec = std::env::var("RADIODOGE_BACKENDS").unwrap_or_default();
    let spec = if spec.trim().is_empty() { "core,blockcypher".to_string() } else { spec };
    parse_backends(&spec, &|k| std::env::var(k).ok())
}

/// Parse a comma-separated backend spec. `env` supplies the per-backend
/// settings (injected so tests do not depend on the process environment).
pub fn parse_backends(spec: &str, env: &dyn Fn(&str) -> Option<String>) -> Result<Vec<Backend>> {
    let mut out = Vec::new();
    for item in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let (name, arg) = match item.split_once('=') {
            Some((n, a)) => (n.trim().to_ascii_lowercase(), Some(a.trim().to_string())),
            None => (item.to_ascii_lowercase(), None),
        };
        let b = match name.as_str() {
            "core" | "rpc" | "dogecoind" => {
                let url = arg
                    .or_else(|| env("RADIODOGE_RPC_URL"))
                    .unwrap_or_else(|| DEFAULT_RPC_URL.to_string());
                let auth = match (env("RADIODOGE_RPC_USER"), env("RADIODOGE_RPC_PASSWORD")) {
                    (Some(user), Some(password)) => RpcAuth::UserPass { user, password },
                    _ => match env("RADIODOGE_RPC_COOKIE").map(std::path::PathBuf::from).or_else(default_cookie_path) {
                        Some(p) => RpcAuth::Cookie(p),
                        None => RpcAuth::None,
                    },
                };
                Backend::Core { url, auth }
            }
            "blockcypher" => Backend::BlockCypher {
                base: arg
                    .or_else(|| env("RADIODOGE_BLOCKCYPHER_URL"))
                    .unwrap_or_else(|| DEFAULT_BLOCKCYPHER_URL.to_string()),
                token: env("RADIODOGE_BLOCKCYPHER_TOKEN").filter(|t| !t.is_empty()),
            },
            "blockbook" => Backend::Blockbook {
                base: arg.ok_or_else(|| anyhow!("blockbook needs a URL: blockbook=https://host/api/v2"))?,
            },
            other => bail!("Unknown backend '{}' (expected core, blockcypher or blockbook=URL)", other),
        };
        out.push(b);
    }
    if out.is_empty() {
        bail!("No backends configured");
    }
    Ok(out)
}

/// Dogecoin Core's default `.cookie` location for this platform.
pub fn default_cookie_path() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    if cfg!(windows) {
        std::env::var_os("APPDATA").map(|a| std::path::PathBuf::from(a).join("Dogecoin").join(".cookie"))
    } else if cfg!(target_os = "macos") {
        home.map(|h| h.join("Library/Application Support/Dogecoin/.cookie"))
    } else {
        home.map(|h| h.join(".dogecoin/.cookie"))
    }
}

fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .connect_timeout(Duration::from_secs(5))
        .user_agent(concat!("radiodoge/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| anyhow!("HTTP client init failed: {}", e))
}

fn bc_url(base: &str, path: &str, token: &Option<String>, query: &str) -> String {
    let mut u = format!("{}{}", base.trim_end_matches('/'), path);
    let mut q: Vec<String> = Vec::new();
    if !query.is_empty() {
        q.push(query.to_string());
    }
    if let Some(t) = token {
        q.push(format!("token={}", t));
    }
    if !q.is_empty() {
        u.push('?');
        u.push_str(&q.join("&"));
    }
    u
}

async fn http_json(req: reqwest::RequestBuilder, what: &str) -> Result<serde_json::Value> {
    let resp = req.send().await.map_err(|e| anyhow!("{} failed: {}", what, e))?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    let json: Option<serde_json::Value> = serde_json::from_str(&body).ok();
    if !status.is_success() {
        // Surface the server's own error text — callers match on phrases such
        // as "already in block chain" / "already exists".
        let detail = json
            .as_ref()
            .and_then(|j| j.get("error"))
            .map(|e| match e {
                serde_json::Value::String(s) => s.clone(),
                other => other.get("message").and_then(|m| m.as_str()).map(String::from).unwrap_or_else(|| other.to_string()),
            })
            .filter(|s| !s.is_empty() && s != "null");
        return Err(match detail {
            Some(d) => anyhow!("{} failed: HTTP {}: {}", what, status.as_u16(), d),
            None => anyhow!("{} failed: HTTP {}", what, status.as_u16()),
        });
    }
    json.ok_or_else(|| anyhow!("{} response parse failed", what))
}

// ─── Dogecoin Core JSON-RPC ──────────────────────────────────────────────────

async fn rpc(url: &str, auth: &RpcAuth, method: &str, params: serde_json::Value) -> Result<serde_json::Value> {
    let body = serde_json::json!({"jsonrpc": "1.0", "id": "radiodoge", "method": method, "params": params});
    let mut req = client()?.post(url).json(&body);
    match auth {
        RpcAuth::None => {}
        RpcAuth::UserPass { user, password } => req = req.basic_auth(user, Some(password)),
        RpcAuth::Cookie(path) => {
            let cookie = std::fs::read_to_string(path)
                .map_err(|e| anyhow!("Core RPC cookie {} unreadable: {}", path.display(), e))?;
            let (u, p) = cookie.trim().split_once(':').ok_or_else(|| anyhow!("Malformed Core RPC cookie"))?;
            req = req.basic_auth(u, Some(p));
        }
    }
    let resp = req.send().await.map_err(|e| anyhow!("Core RPC {} failed: {}", method, e))?;
    let status = resp.status();
    if status.as_u16() == 401 {
        bail!("Core RPC {} failed: HTTP 401 (check rpcuser/rpcpassword or cookie)", method);
    }
    let text = resp.text().await.unwrap_or_default();
    // Core answers RPC-level errors with HTTP 500 and a JSON body — read it either way.
    let v: serde_json::Value = serde_json::from_str(&text)
        .map_err(|_| anyhow!("Core RPC {} failed: HTTP {}", method, status.as_u16()))?;
    if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
        let msg = err.get("message").and_then(|m| m.as_str()).unwrap_or("unknown error");
        bail!("Core RPC {}: {}", method, msg);
    }
    Ok(v.get("result").cloned().unwrap_or(serde_json::Value::Null))
}

async fn core_require_watched(url: &str, auth: &RpcAuth, address: &str) -> Result<()> {
    let info = rpc(url, auth, "validateaddress", serde_json::json!([address])).await?;
    let mine = info.get("ismine").and_then(|v| v.as_bool()).unwrap_or(false);
    let watch = info.get("iswatchonly").and_then(|v| v.as_bool()).unwrap_or(false);
    if !mine && !watch {
        bail!(
            "address {} is not in the Core wallet (listunspent would report 0); import it once with: dogecoin-cli importaddress {} \"radiodoge\" true",
            address, address
        );
    }
    Ok(())
}

async fn core_utxos(url: &str, auth: &RpcAuth, address: &str) -> Result<Vec<Utxo>> {
    core_require_watched(url, auth, address).await?;
    let list = rpc(url, auth, "listunspent", serde_json::json!([0, 9_999_999, [address]])).await?;
    let arr = list.as_array().ok_or_else(|| anyhow!("Core listunspent: unexpected result"))?;
    arr.iter()
        .map(|u| {
            let amount = u.get("amount").and_then(|a| a.as_f64()).ok_or_else(|| anyhow!("listunspent: missing amount"))?;
            Ok(Utxo {
                txid: u.get("txid").and_then(|t| t.as_str()).ok_or_else(|| anyhow!("listunspent: missing txid"))?.to_string(),
                vout: u.get("vout").and_then(|t| t.as_u64()).ok_or_else(|| anyhow!("listunspent: missing vout"))? as u32,
                value: (amount * 1e8).round() as u64,
                confirmations: u.get("confirmations").and_then(|c| c.as_u64()).unwrap_or(0) as u32,
                coinbase: false,
            })
        })
        .collect()
}

// ─── BlockCypher ─────────────────────────────────────────────────────────────

async fn bc_utxos(base: &str, token: &Option<String>, address: &str) -> Result<Vec<Utxo>> {
    let url = bc_url(base, &format!("/addrs/{}", address), token, "unspentOnly=true&includeScript=false&limit=2000");
    let v = http_json(client()?.get(&url), "UTXO fetch").await?;
    if v.get("hasMore").and_then(|h| h.as_bool()).unwrap_or(false) {
        log::warn!("BlockCypher returned a partial UTXO list for {} (hasMore)", address);
    }
    let mut out = Vec::new();
    for key in ["txrefs", "unconfirmed_txrefs"] {
        for r in v.get(key).and_then(|a| a.as_array()).into_iter().flatten() {
            if r.get("spent").and_then(|s| s.as_bool()).unwrap_or(false) {
                continue;
            }
            out.push(Utxo {
                txid: r.get("tx_hash").and_then(|t| t.as_str()).ok_or_else(|| anyhow!("BlockCypher: missing tx_hash"))?.to_string(),
                vout: r.get("tx_output_n").and_then(|n| n.as_u64()).ok_or_else(|| anyhow!("BlockCypher: missing tx_output_n"))? as u32,
                value: r.get("value").and_then(|n| n.as_u64()).ok_or_else(|| anyhow!("BlockCypher: missing value"))?,
                confirmations: r.get("confirmations").and_then(|c| c.as_u64()).unwrap_or(0) as u32,
                coinbase: false,
            });
        }
    }
    Ok(out)
}

// ─── Blockbook ───────────────────────────────────────────────────────────────

async fn bb_utxos(base: &str, address: &str) -> Result<Vec<Utxo>> {
    let url = format!("{}/utxo/{}", base.trim_end_matches('/'), address);
    let v = http_json(client()?.get(&url), "UTXO fetch").await?;
    let arr = v.as_array().ok_or_else(|| anyhow!("Blockbook: unexpected UTXO response"))?;
    arr.iter()
        .map(|u| {
            Ok(Utxo {
                txid: u.get("txid").and_then(|t| t.as_str()).ok_or_else(|| anyhow!("Blockbook: missing txid"))?.to_string(),
                vout: u.get("vout").and_then(|t| t.as_u64()).ok_or_else(|| anyhow!("Blockbook: missing vout"))? as u32,
                value: u.get("value").and_then(|t| t.as_str()).and_then(|s| s.parse().ok()).ok_or_else(|| anyhow!("Blockbook: bad value"))?,
                confirmations: u.get("confirmations").and_then(|c| c.as_u64()).unwrap_or(0) as u32,
                coinbase: u.get("coinbase").and_then(|c| c.as_bool()).unwrap_or(false),
            })
        })
        .collect()
}

// ─── Per-backend operations ──────────────────────────────────────────────────

/// Spendable outputs (confirmed and unconfirmed) for `address` from one backend.
pub async fn utxos_from(b: &Backend, address: &str) -> Result<Vec<Utxo>> {
    match b {
        Backend::Core { url, auth } => core_utxos(url, auth, address).await,
        Backend::BlockCypher { base, token } => bc_utxos(base, token, address).await,
        Backend::Blockbook { base } => bb_utxos(base, address).await,
    }
}

/// Confirmed balance in koinus from one backend.
pub async fn balance_from(b: &Backend, address: &str) -> Result<u64> {
    match b {
        Backend::Core { url, auth } => {
            // Confirmed = outputs with at least one confirmation.
            Ok(core_utxos(url, auth, address).await?.iter().filter(|u| u.confirmations >= 1).map(|u| u.value).sum())
        }
        Backend::BlockCypher { base, token } => {
            let url = bc_url(base, &format!("/addrs/{}/balance", address), token, "");
            let v = http_json(client()?.get(&url), "Balance fetch").await?;
            v.get("balance").and_then(|b| b.as_u64()).ok_or_else(|| anyhow!("Invalid balance value in BlockCypher response"))
        }
        Backend::Blockbook { base } => {
            let url = format!("{}/address/{}?details=basic", base.trim_end_matches('/'), address);
            let v = http_json(client()?.get(&url), "Balance fetch").await?;
            v.get("balance").and_then(|b| b.as_str()).and_then(|s| s.parse().ok()).ok_or_else(|| anyhow!("Invalid balance value in Blockbook response"))
        }
    }
}

/// Broadcast a raw hex transaction through one backend; returns the txid.
pub async fn broadcast_via(b: &Backend, raw_hex: &str) -> Result<String> {
    match b {
        Backend::Core { url, auth } => {
            let r = rpc(url, auth, "sendrawtransaction", serde_json::json!([raw_hex])).await
                .map_err(|e| anyhow!("Network rejected transaction: {}", e))?;
            r.as_str().map(String::from).ok_or_else(|| anyhow!("Unexpected sendrawtransaction result: {}", r))
        }
        Backend::BlockCypher { base, token } => {
            let url = bc_url(base, "/txs/push", token, "");
            let v = http_json(client()?.post(&url).json(&serde_json::json!({"tx": raw_hex})), "Broadcast")
                .await
                .map_err(|e| {
                    let s = e.to_string();
                    if s.contains("HTTP 400") { anyhow!("Network rejected transaction: {}", s) } else { e }
                })?;
            v.pointer("/tx/hash").and_then(|h| h.as_str()).map(String::from)
                .ok_or_else(|| anyhow!("Unexpected broadcast response: {}", v))
        }
        Backend::Blockbook { base } => {
            let url = format!("{}/sendtx/", base.trim_end_matches('/'));
            let v = http_json(client()?.post(&url).header("Content-Type", "text/plain").body(raw_hex.to_string()), "Broadcast").await?;
            if let Some(txid) = v.get("result").and_then(|r| r.as_str()) {
                Ok(txid.to_string())
            } else if let Some(err) = v.get("error").and_then(|r| r.as_str()) {
                bail!("Network rejected transaction: {}", err)
            } else {
                bail!("Unexpected broadcast response: {}", v)
            }
        }
    }
}

/// Confirmation status of `txid` from one backend.
pub async fn tx_status_from(b: &Backend, txid: &str) -> Result<TxStatus> {
    match b {
        Backend::Core { url, auth } => {
            // Works for mempool txs, wallet txs, and anything if Core runs with txindex=1.
            let v = rpc(url, auth, "getrawtransaction", serde_json::json!([txid, 1])).await?;
            let confirmations = v.get("confirmations").and_then(|c| c.as_u64()).unwrap_or(0) as u32;
            let block_hash = v.get("blockhash").and_then(|h| h.as_str()).map(String::from);
            let block_height = match &block_hash {
                Some(h) => rpc(url, auth, "getblockheader", serde_json::json!([h])).await?
                    .get("height").and_then(|x| x.as_u64()),
                None => None,
            };
            Ok(TxStatus { confirmations, block_height, block_hash })
        }
        Backend::BlockCypher { base, token } => {
            let url = bc_url(base, &format!("/txs/{}", txid), token, "includeHex=false&limit=1");
            let v = http_json(client()?.get(&url), "Transaction lookup").await?;
            let h = v.get("block_height").and_then(|x| x.as_i64()).unwrap_or(-1);
            Ok(TxStatus {
                confirmations: v.get("confirmations").and_then(|c| c.as_u64()).unwrap_or(0) as u32,
                block_height: (h >= 0).then_some(h as u64),
                block_hash: v.get("block_hash").and_then(|x| x.as_str()).map(String::from),
            })
        }
        Backend::Blockbook { base } => {
            let url = format!("{}/tx/{}", base.trim_end_matches('/'), txid);
            let v = http_json(client()?.get(&url), "Transaction lookup").await?;
            let h = v.get("blockHeight").and_then(|x| x.as_i64()).unwrap_or(-1);
            Ok(TxStatus {
                confirmations: v.get("confirmations").and_then(|c| c.as_u64()).unwrap_or(0) as u32,
                block_height: (h >= 0).then_some(h as u64),
                block_hash: v.get("blockHash").and_then(|x| x.as_str()).map(String::from),
            })
        }
    }
}

// ─── Fallback over the configured list ───────────────────────────────────────

/// Run `op` against each backend in order and return the first success.
/// On total failure the error lists every backend's reason, so phrase-matching
/// callers (e.g. "already in block chain") still see the server text.
pub async fn with_fallback<T, F, Fut>(backends: &[Backend], what: &str, op: F) -> Result<T>
where
    F: Fn(Backend) -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let mut errors = Vec::new();
    for b in backends {
        match op(b.clone()).await {
            Ok(v) => {
                log::debug!("{} served by {}", what, b.name());
                return Ok(v);
            }
            Err(e) => {
                log::info!("{} via {} failed: {}", what, b.name(), e);
                errors.push(format!("{}: {}", b.name(), e));
            }
        }
    }
    bail!("{} — all backends failed: {}", what, errors.join(" | "))
}

pub async fn fetch_balance(address: &str) -> Result<u64> {
    let bs = configured_backends()?;
    with_fallback(&bs, "Balance fetch", |b| async move { balance_from(&b, address).await }).await
}

pub async fn fetch_utxos(address: &str) -> Result<Vec<Utxo>> {
    let bs = configured_backends()?;
    with_fallback(&bs, "UTXO fetch", |b| async move { utxos_from(&b, address).await }).await
}

pub async fn broadcast(raw_hex: &str) -> Result<String> {
    let bs = configured_backends()?;
    with_fallback(&bs, "Broadcast", |b| async move { broadcast_via(&b, raw_hex).await }).await
}

pub async fn fetch_tx_status(txid: &str) -> Result<TxStatus> {
    let bs = configured_backends()?;
    with_fallback(&bs, "Transaction lookup", |b| async move { tx_status_from(&b, txid).await }).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Minimal one-shot-per-connection HTTP mock: serves `responses` in order
    /// and returns the raw requests it saw.
    async fn mock(responses: Vec<(u16, &'static str)>) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let h = tokio::spawn(async move {
            let mut seen = Vec::new();
            for (code, body) in responses {
                let (mut s, _) = listener.accept().await.unwrap();
                let mut buf = vec![0u8; 65536];
                let mut req = Vec::new();
                loop {
                    let n = s.read(&mut buf).await.unwrap();
                    req.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&req).to_string();
                    if let Some(i) = text.find("\r\n\r\n") {
                        let len = text.lines()
                            .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
                            .unwrap_or(0);
                        if req.len() >= i + 4 + len { break; }
                    }
                    if n == 0 { break; }
                }
                seen.push(String::from_utf8_lossy(&req).to_string());
                let resp = format!("HTTP/1.1 {} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", code, body.len(), body);
                s.write_all(resp.as_bytes()).await.unwrap();
                let _ = s.shutdown().await;
            }
            seen
        });
        (url, h)
    }

    fn no_env(_: &str) -> Option<String> { None }

    #[test]
    fn parse_defaults_and_overrides() {
        let b = parse_backends("core,blockcypher", &no_env).unwrap();
        assert_eq!(b.len(), 2);
        assert!(matches!(&b[0], Backend::Core { url, .. } if url == DEFAULT_RPC_URL));
        assert!(matches!(&b[1], Backend::BlockCypher { base, token: None } if base == DEFAULT_BLOCKCYPHER_URL));

        let env = |k: &str| match k {
            "RADIODOGE_RPC_USER" => Some("u".into()),
            "RADIODOGE_RPC_PASSWORD" => Some("p".into()),
            "RADIODOGE_BLOCKCYPHER_TOKEN" => Some("t".into()),
            _ => None,
        };
        let b = parse_backends("core=http://10.0.0.2:22555, blockcypher, blockbook=https://bb/api/v2", &env).unwrap();
        assert_eq!(b[0], Backend::Core { url: "http://10.0.0.2:22555".into(), auth: RpcAuth::UserPass { user: "u".into(), password: "p".into() } });
        assert_eq!(b[1], Backend::BlockCypher { base: DEFAULT_BLOCKCYPHER_URL.into(), token: Some("t".into()) });
        assert_eq!(b[2], Backend::Blockbook { base: "https://bb/api/v2".into() });

        assert!(parse_backends("blockbook", &no_env).is_err(), "blockbook needs an explicit URL");
        assert!(parse_backends("trezor", &no_env).is_err());
        assert!(parse_backends(" , ", &no_env).is_err());
    }

    #[test]
    fn trezor_is_not_a_default() {
        let b = parse_backends("core,blockcypher", &no_env).unwrap();
        assert!(b.iter().all(|x| !matches!(x, Backend::Blockbook { .. })));
    }

    #[tokio::test]
    async fn blockcypher_balance_utxos_broadcast() {
        let (url, h) = mock(vec![
            (200, r#"{"address":"D","balance":2300000000,"unconfirmed_balance":0}"#),
            (200, r#"{"txrefs":[{"tx_hash":"aa","tx_output_n":1,"value":500,"confirmations":7,"spent":false},{"tx_hash":"bb","tx_output_n":0,"value":9,"confirmations":3,"spent":true}],"unconfirmed_txrefs":[{"tx_hash":"cc","tx_output_n":2,"value":42,"confirmations":0}]}"#),
            (201, r#"{"tx":{"hash":"deadbeef"}}"#),
            (400, r#"{"error":"Error validating transaction: Transaction with hash deadbeef already exists."}"#),
        ]).await;
        let b = Backend::BlockCypher { base: url, token: Some("tok".into()) };
        assert_eq!(balance_from(&b, "D").await.unwrap(), 2_300_000_000);
        let u = utxos_from(&b, "D").await.unwrap();
        assert_eq!(u, vec![
            Utxo { txid: "aa".into(), vout: 1, value: 500, confirmations: 7, coinbase: false },
            Utxo { txid: "cc".into(), vout: 2, value: 42, confirmations: 0, coinbase: false },
        ]);
        assert_eq!(broadcast_via(&b, "0100").await.unwrap(), "deadbeef");
        let e = broadcast_via(&b, "0100").await.unwrap_err().to_string();
        assert!(e.contains("Network rejected transaction") && e.contains("already exists"), "{e}");
        let reqs = h.await.unwrap();
        assert!(reqs[0].starts_with("GET /addrs/D/balance?token=tok "));
        assert!(reqs[1].contains("unspentOnly=true"));
        assert!(reqs[2].starts_with("POST /txs/push?token=tok") && reqs[2].contains(r#"{"tx":"0100"}"#));
    }

    #[tokio::test]
    async fn core_rpc_paths() {
        let (url, h) = mock(vec![
            // balance: validateaddress + listunspent
            (200, r#"{"result":{"isvalid":true,"ismine":false,"iswatchonly":true},"error":null,"id":"radiodoge"}"#),
            (200, r#"{"result":[{"txid":"aa","vout":0,"amount":23.0,"confirmations":2},{"txid":"bb","vout":1,"amount":0.5,"confirmations":0}],"error":null,"id":"radiodoge"}"#),
            // not imported → explicit error instead of a fake zero
            (200, r#"{"result":{"isvalid":true,"ismine":false,"iswatchonly":false},"error":null,"id":"radiodoge"}"#),
            // broadcast ok, then rejection (Core uses HTTP 500 + JSON error)
            (200, r#"{"result":"cafe","error":null,"id":"radiodoge"}"#),
            (500, r#"{"result":null,"error":{"code":-27,"message":"transaction already in block chain"},"id":"radiodoge"}"#),
        ]).await;
        let b = Backend::Core { url, auth: RpcAuth::UserPass { user: "u".into(), password: "p".into() } };
        assert_eq!(balance_from(&b, "D").await.unwrap(), 2_300_000_000);
        let e = balance_from(&b, "D").await.unwrap_err().to_string();
        assert!(e.contains("importaddress"), "{e}");
        assert_eq!(broadcast_via(&b, "0100").await.unwrap(), "cafe");
        let e = broadcast_via(&b, "0100").await.unwrap_err().to_string();
        assert!(e.contains("already in block chain"), "{e}");
        let reqs = h.await.unwrap();
        assert!(reqs[0].to_ascii_lowercase().contains("authorization: basic dtpw")); // base64("u:p")
        assert!(reqs[1].contains(r#""method":"listunspent""#));
        assert!(reqs[3].contains(r#""method":"sendrawtransaction""#));
    }

    #[tokio::test]
    async fn falls_back_when_primary_is_down() {
        // Port 9 on localhost: nothing listening → connection refused.
        let (url, h) = mock(vec![(200, r#"{"balance":7}"#)]).await;
        let list = vec![
            Backend::Core { url: "http://127.0.0.1:9".into(), auth: RpcAuth::None },
            Backend::Blockbook { base: url.clone() },
        ];
        // Blockbook returns balance as a string; a number is a parse error → both fail.
        let e = with_fallback(&list, "Balance fetch", |b| async move { balance_from(&b, "D").await }).await.unwrap_err().to_string();
        assert!(e.contains("core:") && e.contains("blockbook:"), "{e}");
        h.await.unwrap();

        let (url, h) = mock(vec![(200, r#"{"balance":"7"}"#)]).await;
        let list = vec![
            Backend::Core { url: "http://127.0.0.1:9".into(), auth: RpcAuth::None },
            Backend::Blockbook { base: url },
        ];
        assert_eq!(with_fallback(&list, "Balance fetch", |b| async move { balance_from(&b, "D").await }).await.unwrap(), 7);
        h.await.unwrap();
    }

    #[tokio::test]
    async fn tx_status_blockcypher() {
        let (url, h) = mock(vec![(200, r#"{"block_height":6405433,"block_hash":"00ff","confirmations":3}"#)]).await;
        let b = Backend::BlockCypher { base: url, token: None };
        let s = tx_status_from(&b, "ab").await.unwrap();
        assert_eq!(s, TxStatus { confirmations: 3, block_height: Some(6405433), block_hash: Some("00ff".into()) });
        h.await.unwrap();
    }
}
