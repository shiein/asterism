# Asterism

[English](README.md) · [简体中文](README.zh-CN.md)

Windows / macOS clipboard sync, screenshot, and a self-hosted Linux Hub.

Design baseline: [docs/design/windows_macos_clipboard_capture_hub_final_design.md](docs/design/windows_macos_clipboard_capture_hub_final_design.md)

## What it is

Asterism is a personal productivity stack:

| Piece | Role |
|---|---|
| **Desktop** | Windows / macOS client: clipboard sync, screenshot / recording, local history |
| **Hub** | Single Linux binary: relay, encrypted history, blobs, devices, embedded Web |
| **Web** | History center only. It does **not** watch the system clipboard |

LAN is preferred. Remote traffic goes through your Hub. The Hub stores ciphertext by default; search runs on the client.

```text
Windows Desktop  ◄── LAN TLS ──►  macOS Desktop
        │                              │
        └──────── HTTPS / WSS ─────────┘
                        │
                   Linux Hub
                   (single binary)
                        │
                   Web history
```

## V1 scope

**Does**

- Text, generic images, files / folders between Windows and macOS
- LAN Direct (mDNS + Hub-assisted candidates), with fallback to Hub
- E2EE remote payloads (AVK / item keys). Hub does not see plaintext
- Sensitive clipboard is ignored by default (no history, no sync, no upload)
- Region / window / fullscreen screenshot, annotation, scroll capture, GIF, video
- Embedded Web history with local search after vault unlock

**Does not (V1)**

- Linux desktop client, mobile apps, browser clipboard extension
- NAT traversal / WebRTC
- Shipping OCR / AI / remote desktop (interfaces only)
- Docker as a required dependency
- Extra infra (PostgreSQL, Redis, object storage, …)

## Repository

Cargo workspace + pnpm workspace.

| Path | Responsibility |
|---|---|
| `crates/core` | Content / Action / Device / Policy |
| `crates/crypto` | Hash, E2EE, key wrap (shared with future WASM) |
| `crates/storage` | SQLite WAL + single writer queue + local blobs |
| `crates/clipboard` | System clipboard, watcher, normalize / dedup / policy |
| `crates/capture` | Overlay, screenshot, scroll |
| `crates/media` | GIF / video / audio |
| `crates/sync` | Protocol, LAN TLS, Hub client |
| `hub` | `asterism-hub` single binary |
| `desktop` | Tauri 2 + React |
| `web` | Embedded Web history UI |
| `deploy/systemd` | Official unit. No Docker tree |

## Requirements

- Rust stable (`rust-toolchain.toml`; rust-version **1.88**)
- Node **22+** and **pnpm 10**
- Desktop: Windows 10+ or macOS 13+
- Hub: Linux x86_64 (or build from source)

## Develop

```bash
export PATH="$HOME/.cargo/bin:$PATH"
pnpm install

cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

### Hub (local)

```bash
cargo run -p asterism-hub -- init --data-dir ./data
cargo run -p asterism-hub -- serve --config ./data/config.toml
```

`init` prints a **bootstrap secret** once and writes `data/bootstrap.secret`. Save it. The first Desktop uses that secret; later devices use a pairing code from an already-registered Desktop.

Default TLS is a self-signed cert for `localhost` / `127.0.0.1`. The Desktop **TOFU-pins** the Hub certificate fingerprint on first successful handshake (`hub_cert_sha256` in `sync.toml`). If you rotate the cert, clear that field and pair again. Do not enable “accept any certificate”.

Useful commands:

```bash
cargo run -p asterism-hub -- migrate --config ./data/config.toml
cargo run -p asterism-hub -- doctor --config ./data/config.toml
cargo run -p asterism-hub -- backup --config ./data/config.toml --dest ./backup.db
```

### Desktop

```bash
pnpm desktop:dev
# or, from desktop/
pnpm tauri dev
```

In Settings:

1. Set Hub URL, e.g. `https://127.0.0.1:8787`
2. First device: paste the bootstrap secret → **Connect and register this machine**
3. Later device: generate a pairing code on the first machine, attach AVK to the code, then connect with that code

Keep the Recovery Key offline. It is the AVK backup.

### Web

The Hub serves the built React app. For UI-only work:

```bash
pnpm web:dev
```

Web never listens to the OS clipboard. Unlock the vault with the Recovery Key before search / copy / download.

## Deploy Hub (Linux)

Release artifact is one binary plus a data directory. Docker is not required.

```text
/opt/asterism/
├── asterism-hub
└── data/
    ├── hub.db
    ├── blobs/
    ├── config.toml
    ├── tls.cert
    └── tls.key
```

```bash
cargo build -p asterism-hub --release
./deploy/install-hub.sh
# copy deploy/systemd/asterism-hub.service, then:
# systemctl enable --now asterism-hub
```

Upgrade: stop the unit, replace the binary atomically, start. `serve` / `migrate` run controlled SQLite migrations.

## Security baseline (short)

- Hub is HTTPS / WSS only
- Remote payloads are E2EE; filenames are metadata, never server paths
- Pairing codes are one-shot and expire (10 minutes)
- Device identity can be revoked; LAN trust is dropped on revoke
- Receive paths reject traversal and do not follow symlink / junction
- Logs must never contain clipboard body, file bytes, or keys

## Status

Phases 1–5 of the design baseline are wired (clipboard, LAN, Hub + Web + E2EE, screenshot, scroll / GIF / video). Phase 6 hardening (signed installers, soak, formal perf gates) is not done.

LAN / Hub dual-machine behavior needs to be checked on your own Windows + macOS pair.

## License

[MIT](LICENSE)
