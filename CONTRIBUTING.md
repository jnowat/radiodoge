# 🤝 Contributing to RadioDoge

Thanks for wanting to make RadioDoge better! Whether it's code, docs, hardware testing, or a bug report, it's
all welcome. *Much collaboration. Very wow.* 🐕

---

## Ways to help

- **🐛 Report bugs** — [open an issue](https://github.com/jnowat/RadioDoge/issues) with steps to reproduce, your
  OS/app version, and (if hardware-related) your Heltec board and firmware version (`GET_FIRMWARE_VERSION` or the
  version badge in the app).
- **📡 Test with real hardware** — the app is only as good as its field testing. Range tests, driver quirks, and
  Android device reports are especially valuable.
- **📝 Improve the docs** — see [`docs/`](docs/). Small fixes (typos, clearer wording) are perfect first PRs.
- **✨ Add features** — check the [Roadmap](ROADMAP.md) first so we don't duplicate work.

---

## Project layout

RadioDoge is a Cargo workspace plus Arduino firmware. See the
[Architecture section of the README](README.md#architecture) for the full map. The pieces you'll touch most:

| Area | Where |
|------|-------|
| Shared protocol / wallet / serial | `crates/radiodoge-core/src/` |
| Headless CLI + gateway daemon | `crates/radiodoge-cli/src/main.rs` |
| Desktop/Android app frontend | `radiodoge-gui/src/` (Svelte 5 + TypeScript) |
| App backend | `radiodoge-gui/src-tauri/src/` (Rust) |
| Firmware | `heltec-firmware-v3/` (Arduino) |

---

## Development setup

```bash
# Prerequisites: Rust (https://rustup.rs) and Node.js 20+ (https://nodejs.org)

# Build & test the Rust workspace
cargo build --workspace
cargo test  --workspace

# Run the GUI with hot-reloading
cd radiodoge-gui
npm ci
cargo tauri dev

# Build the CLI
cargo build -p radiodoge-cli --release
```

For firmware, open `heltec-firmware-v3/heltec-firmware.ino` in Arduino IDE 2.x — see the
[flashing guide](README.md#-flashing-the-heltec-firmware).

---

## Submitting a change

1. **Fork** the repository and create a feature branch:
   ```bash
   git checkout -b feature/much-wow
   ```
2. **Make your change.** Match the surrounding style — the frontend follows the existing Svelte/TypeScript
   conventions, and Rust should be `cargo fmt`-clean.
3. **Test it.** Run `cargo test --workspace`. If you changed runtime behavior, exercise it in the app or CLI, not
   just the tests.
4. **Commit** with a clear, descriptive message (Conventional-Commits style is appreciated — `fix:`, `feat:`,
   `docs:`, `ci:`).
5. **Push and open a Pull Request** describing what changed and why. Link any related issue.

CI will build the Windows MSI and Android APK and run `cargo-audit`; please make sure those are green.

---

## Coding conventions

- **Rust** — keep protocol/wallet logic in `radiodoge-core` so the app, CLI, and mobile bridge all share one
  implementation. Prefer `cargo fmt` defaults; avoid adding native dependencies (the wallet is intentionally
  pure Rust).
- **Frontend** — Svelte 5 runes, TypeScript, TailwindCSS v4. Reuse the existing stores in `src/lib/stores/`.
- **Docs** — keep them accurate. If you change behavior, update the [User Manual](docs/USER_MANUAL.md),
  [Protocol reference](docs/PROTOCOL.md), and [Changelog](CHANGELOG.md) as needed.
- **Security** — never log or transmit a private key or recovery phrase. Flag anything that touches signing,
  encryption, or the firmware web interface in your PR description.

---

## Code of conduct

Be kind, be constructive, assume good faith. We're here to build something fun and useful for the Dogecoin
community. 🐕🌙

---

*Such contribution. Very appreciated. Wow.*
