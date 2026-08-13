# Asterism

[English](README.md) · [简体中文](README.zh-CN.md)

Sync clipboard, screenshots, and files between Windows and macOS. A small self-hosted Hub covers remote sync and a Web history page.

## Features

- Sync **text**, **images**, and **files / folders** between Windows and macOS
- Prefer **LAN direct** (encrypted). If the LAN path is down, traffic goes through your Hub
- Remote items are **end-to-end encrypted**. The Hub stores ciphertext; search happens on the device
- **Sensitive clipboard** (password managers, system “do not sync” flags) stays local by default
- **Screenshot**: region, window, fullscreen, annotation, scrolling capture
- **GIF** and **video** recording
- **Web history** in the Hub: browse, search, copy or download. The browser never watches the system clipboard

```text
Windows  ◄── LAN ──►  macOS
    │                   │
    └──── your Hub ─────┘
              │
         Web history
```

## Use

You need Rust (stable) and Node 22+ with pnpm.

### 1. Start the Hub

On a Linux machine, or on your Mac for a local test:

```bash
cargo run -p asterism-hub -- init --data-dir ./data
cargo run -p asterism-hub -- serve --config ./data/config.toml
```

`init` prints a **bootstrap secret** once (also written to `data/bootstrap.secret`). Save it. The first computer uses this secret; later computers use a pairing code.

The Hub listens on `https://127.0.0.1:8787` by default and uses a self-signed certificate. The desktop app remembers that certificate on first connect. If you later replace the certificate, remove `hub_cert_sha256` from the desktop `sync.toml` and connect again.

On a Linux server:

```bash
cargo build -p asterism-hub --release
./deploy/install-hub.sh
# install deploy/systemd/asterism-hub.service, then:
# sudo systemctl enable --now asterism-hub
```

### 2. Open the desktop app

```bash
pnpm install
pnpm desktop:dev
```

Settings:

1. Hub URL, for example `https://127.0.0.1:8787`
2. **First computer**: paste the bootstrap secret → Connect and register this machine
3. **Next computer**: on the first machine, generate a pairing code, attach the vault key (AVK) to that code, then connect with the code

Keep the **Recovery Key** offline. It is the backup of your vault.

Copy as usual. History appears in the app. Use the capture actions for screenshot, scroll, GIF, or video.

### 3. Open Web history

Visit the Hub URL in a browser (the Hub serves the page). Pair with a code from the desktop app, then unlock with the Recovery Key. You can search, copy text, and download images or files. The page does not read the system clipboard.

## License

[MIT](LICENSE)
