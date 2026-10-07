# Chain backends (balance, UTXOs, broadcast)

RadioDoge signs transactions locally, but it still needs the Dogecoin network for three things:
an address's **confirmed balance**, its **spendable outputs (UTXOs)**, and **broadcasting** a signed
transaction (plus the optional `verify-tx` confirmation check). Since v0.4.3 these go through an
ordered list of backends; the first one that answers wins, and the rest are fallbacks.

| Backend | Default? | Balance / UTXOs | Broadcast | Notes |
|---------|----------|-----------------|-----------|-------|
| `core` — your own Dogecoin Core node (JSON-RPC) | **1st** | `validateaddress` + `listunspent` | `sendrawtransaction` | Most private; nothing leaves your machine |
| `blockcypher` — `api.blockcypher.com/v1/doge/main` | 2nd (fallback) | `/addrs/{addr}/balance`, `/addrs/{addr}?unspentOnly=true` | `/txs/push` | Rate-limited without a token; leaks your address to a third party |
| `blockbook=URL` — any Blockbook v2 server | opt-in only | `/address`, `/utxo` | `/sendtx` | Trezor's public `doge1.trezor.io` now answers API clients with HTTP 403 and is **no longer used** |

## Configuration

Environment variables (the CLI flags in brackets override them):

| Variable | Meaning | Default |
|----------|---------|---------|
| `RADIODOGE_BACKENDS` (`--backends`) | comma list, e.g. `core,blockcypher` | `core,blockcypher` |
| `RADIODOGE_RPC_URL` (`--rpc-url`) | Core RPC URL | `http://127.0.0.1:22555` |
| `RADIODOGE_RPC_USER` (`--rpc-user`) | `rpcuser` | — |
| `RADIODOGE_RPC_PASSWORD` | `rpcpassword` (env only — never on the command line) | — |
| `RADIODOGE_RPC_COOKIE` (`--rpc-cookie`) | path to Core's `.cookie` | `%APPDATA%\Dogecoin\.cookie`, `~/.dogecoin/.cookie`, `~/Library/Application Support/Dogecoin/.cookie` |
| `RADIODOGE_BLOCKCYPHER_URL` | BlockCypher base URL | `https://api.blockcypher.com/v1/doge/main` |
| `RADIODOGE_BLOCKCYPHER_TOKEN` | BlockCypher API token (higher quota) | — |

Examples:

```bash
radiodoge-cli balance -a D...                                  # core, then blockcypher
radiodoge-cli --backends blockcypher balance -a D...           # public API only
radiodoge-cli --backends core=http://192.168.1.10:22555 broadcast ...
```

The GUI reads the same environment variables; the `set_chain_backends` command overrides the list
for the running session (empty string = back to defaults).

## Using your own Dogecoin Core node

1. **Enable RPC.** `dogecoin-qt` does not serve RPC unless told to. Add to `dogecoin.conf`
   (Windows: `%APPDATA%\Dogecoin\dogecoin.conf`) and restart Core:
   ```ini
   server=1
   # Leave rpcuser/rpcpassword unset to use cookie auth (Core writes .cookie on start),
   # or set rpcauth=... (generate with share/rpcauth/rpcauth.py) and export
   # RADIODOGE_RPC_USER / RADIODOGE_RPC_PASSWORD.
   # Keep RPC on localhost unless you know what you are doing:
   rpcbind=127.0.0.1
   rpcallowip=127.0.0.1
   ```
2. **Broadcast works immediately** (`sendrawtransaction`).
3. **Balance and UTXOs need the address in Core's wallet.** Dogecoin Core 1.14 has no
   `scantxoutset` and no address index, so `listunspent` only sees wallet addresses. Import the
   RadioDoge address once as **watch-only** (no private key goes into Core):
   ```bash
   dogecoin-cli importaddress D... "radiodoge" true    # true = rescan (slow, one time)
   ```
   Until then the `core` backend reports *"address … is not in the Core wallet"* (never a fake 0)
   and the next backend is used.
4. **`verify-tx`** uses `getrawtransaction`, which finds mempool and wallet transactions; for any
   transaction set `txindex=1` (requires a one-time reindex).

## Caveats

- BlockCypher and Core's `listunspent` do not flag coinbase outputs, so the immature-coinbase guard
  only applies to Blockbook data. Ordinary wallets never receive coinbase outputs.
- Unconfirmed outputs are never spent, whichever backend reported them.
