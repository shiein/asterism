# Asterism

Windows / macOS 跨设备剪贴板、截图与自托管 Hub。

开发基线：[docs/design/windows_macos_clipboard_capture_hub_final_design.md](docs/design/windows_macos_clipboard_capture_hub_final_design.md)

## 组成

```text
Windows Desktop  ◄── LAN TLS ──►  macOS Desktop
        │                              │
        └──────── HTTPS / WSS ─────────┘
                        │
                   Linux Hub
                   (单二进制)
                        │
                   Web 历史中心
```

- **Desktop**：Tauri 2 + React，系统能力全部在 Rust。
- **Hub**：`asterism-hub`，Axum + rustls + SQLite + 本地文件系统，不依赖 Docker。
- **Web**：只做历史浏览 / 搜索 / 下载，不监听系统剪贴板。

## 当前进度

代码层已按基线阶段接通主路径（现场验收仍需你在真实双机上跑）：

1. Core / 本地历史 / Windows+macOS 剪贴板 / 敏感策略 / Desktop UI  
2. 设备证书、mDNS、LAN TCP+TLS、文件分块流、Hub client  
3. Hub 配对/会话/设备/密文历史/Blob/WSS 中继；LAN Direct 真传；文件归档跨端；AVK 经配对码包装传递  
4. 窗口/选区截图、标注 Undo/导出、滚动拼接  
5. GIF / Motion-JPEG AVI 录制入口  
6. 崩溃锁、缓存淘汰、Hub 安装脚本、macOS 开机启动

## 开发

需要 Rust stable（见 `rust-toolchain.toml`）和 Node 22+ / pnpm。

```bash
export PATH="$HOME/.cargo/bin:$PATH"

cargo test --workspace
cargo run -p asterism-hub -- --help

pnpm install
pnpm desktop:dev
```

Hub 本地初始化：

```bash
cargo run -p asterism-hub -- init --data-dir ./data
cargo run -p asterism-hub -- serve --config ./data/config.toml
```

## 部署（Hub）

发布物是单一 Linux 二进制，配合 `deploy/systemd/asterism-hub.service`。

```text
/opt/asterism/
├── asterism-hub
└── data/
    ├── hub.db
    ├── blobs/
    └── config.toml
```
