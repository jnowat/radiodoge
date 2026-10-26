# radiodoge-inspector

Decode RadioDoge packets from a hex dump, in the browser. The decoder is the
protocol half of `radiodoge-core` (`default-features = false`: no Tokio, serial,
crypto or HTTP) compiled to WebAssembly, so packets are framed and decoded
exactly like the desktop app and CLI do it. Nothing leaves the page.

## Build

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.<match Cargo.lock>
cargo build -p radiodoge-inspector --release --target wasm32-unknown-unknown
wasm-bindgen --target web --out-dir crates/radiodoge-inspector/www/pkg \
  target/wasm32-unknown-unknown/release/radiodoge_inspector.wasm
```

## Run

Serve `www/` over HTTP (browsers refuse to load wasm from `file://`):

```sh
python -m http.server -d crates/radiodoge-inspector/www 8080
# open http://localhost:8080
```

Link straight to a dump with `index.html#hex=02000a0001...`; it decodes on load.

Input may use spaces, commas, colons, dashes or `0x` prefixes between bytes.
Bytes that cannot start a packet are listed as skipped noise; an incomplete
packet at the end is reported as trailing bytes.

Without the `native` feature, `DOGE_TX` payloads show their length only;
signature checks need the desktop app or CLI.
