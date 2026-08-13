# Asterism

[English](README.md) · [简体中文](README.zh-CN.md)

Windows / macOS 跨设备剪贴板、截图，以及自托管 Linux Hub。

开发基线：[docs/design/windows_macos_clipboard_capture_hub_final_design.md](docs/design/windows_macos_clipboard_capture_hub_final_design.md)

## 它是什么

Asterism 是一套个人效率工具：

| 部分 | 职责 |
|---|---|
| **Desktop** | Windows / macOS 客户端：剪贴板同步、截图 / 录屏、本地历史 |
| **Hub** | 单个 Linux 二进制：中转、密文历史、Blob、设备管理、内嵌 Web |
| **Web** | 只做历史中心，**不**监听系统剪贴板 |

局域网优先直连。远程走自托管 Hub。Hub 默认只存密文，搜索在客户端。

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

## V1 范围

**做**

- Windows 与 macOS 之间同步文字、通用图片、文件 / 文件夹
- LAN Direct（mDNS + Hub 协助发现），失败后降级 Hub
- 远程 payload 默认 E2EE（AVK / ItemKey）。Hub 看不见明文
- 敏感剪贴板默认忽略（不进历史、不同步、不上传）
- 区域 / 窗口 / 全屏截图、标注、滚动截图、GIF、视频
- 内嵌 Web 历史；解锁 Vault 后在本地搜索

**不做（V1）**

- Linux 桌面客户端、移动端、浏览器剪贴板扩展
- NAT 穿透 / WebRTC
- 正式 OCR / AI / 远程桌面（只预留接口）
- 把 Docker 当作部署依赖
- PostgreSQL、Redis、对象存储等额外基础设施

## 仓库

Cargo workspace + pnpm workspace。

| 路径 | 职责 |
|---|---|
| `crates/core` | Content / Action / Device / Policy |
| `crates/crypto` | Hash、E2EE、Key wrap（与未来 WASM 共用） |
| `crates/storage` | SQLite WAL + Single Writer Queue + 本地 Blob |
| `crates/clipboard` | 系统剪贴板、Watcher、Normalize / Dedup / Policy |
| `crates/capture` | Overlay、截图、滚动 |
| `crates/media` | GIF / 视频 / 音频 |
| `crates/sync` | 协议、LAN TLS、Hub client |
| `hub` | `asterism-hub` 单二进制 |
| `desktop` | Tauri 2 + React |
| `web` | Hub 内嵌的 Web 历史界面 |
| `deploy/systemd` | 正式部署单元，没有 Docker 目录 |

## 环境

- Rust stable（见 `rust-toolchain.toml`；rust-version **1.88**）
- Node **22+** 与 **pnpm 10**
- Desktop：Windows 10+ 或 macOS 13+
- Hub：Linux x86_64（或自行从源码编译）

## 开发

```bash
export PATH="$HOME/.cargo/bin:$PATH"
pnpm install

cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

### Hub（本地）

```bash
cargo run -p asterism-hub -- init --data-dir ./data
cargo run -p asterism-hub -- serve --config ./data/config.toml
```

`init` 会打印一次 **bootstrap secret**，并写入 `data/bootstrap.secret`。请自行保存。第一台 Desktop 用这个 secret；之后的设备用已注册 Desktop 生成的配对码。

默认 TLS 是仅含 `localhost` / `127.0.0.1` 的自签名证书。Desktop 在首次成功握手时 **TOFU pin** Hub 证书指纹（`sync.toml` 里的 `hub_cert_sha256`）。轮换证书后清掉该字段再配对。不要打开「接受任意证书」。

常用命令：

```bash
cargo run -p asterism-hub -- migrate --config ./data/config.toml
cargo run -p asterism-hub -- doctor --config ./data/config.toml
cargo run -p asterism-hub -- backup --config ./data/config.toml --dest ./backup.db
```

### Desktop

```bash
pnpm desktop:dev
# 或在 desktop/ 下
pnpm tauri dev
```

设置页：

1. 填写 Hub URL，例如 `https://127.0.0.1:8787`
2. 第一台设备：粘贴 bootstrap secret → **连接并注册本机**
3. 后续设备：在第一台生成配对码，把 AVK 附到配对码上，再用该码连接

Recovery Key 请离线保管，它是 AVK 的备份。

### Web

Hub 会托管构建后的 React 应用。只改界面时：

```bash
pnpm web:dev
```

Web 不监听系统剪贴板。搜索 / 复制 / 下载前先用 Recovery Key 解锁 Vault。

## 部署 Hub（Linux）

发布物是单个二进制 + 数据目录，不依赖 Docker。

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
# 复制 deploy/systemd/asterism-hub.service 后：
# systemctl enable --now asterism-hub
```

升级：停服务，原子替换二进制，再启动。`serve` / `migrate` 会执行受控的 SQLite migration。

## 安全基线（摘要）

- Hub 只走 HTTPS / WSS
- 远程 payload 默认 E2EE；文件名只作 metadata，不会拼成服务器路径
- 配对码一次性，10 分钟过期
- 设备身份可撤销；撤销后回收 LAN 信任
- 接收路径拒绝 Path Traversal，不跟随 symlink / junction
- 日志不得记录剪贴板正文、文件内容或密钥

## 当前进度

设计基线 Phase 1–5 的主路径已接通（剪贴板、LAN、Hub + Web + E2EE、截图、滚动 / GIF / 视频）。Phase 6 加固（签名安装包、soak、正式性能门禁）尚未做。

LAN / Hub 双机行为需要你在自己的 Windows + macOS 上验收。

## 许可

[MIT](LICENSE)
