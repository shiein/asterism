# Asterism — 项目级指令

> 产品范围与阶段以 `docs/design/windows_macos_clipboard_capture_hub_final_design.md` 为基线。
> 内部组合方式以 `docs/design/asterism_kernel_plugins_refactoring_plan.md` 为重构基线（Kernel + Sealed Domain + 静态插件）。
> 当前任务的明确要求 > 本文 > 全局偏好。
> 重构不得削弱产品硬约束；未完成阶段不得用双事实源并行。

## 产品

Asterism 是 Windows/macOS 桌面效率客户端 + Linux 自托管 Hub + Web 历史中心。

- Desktop：剪贴板同步、截图/录屏、本地历史
- Hub：单 Linux 二进制，远程中转、历史、Blob、设备管理、内嵌 Web
- Web：历史中心，不监听系统剪贴板
- V1 不做：Linux Desktop、移动端、浏览器扩展、NAT/WebRTC、正式 AI/OCR/远程桌面、Docker 依赖

## 仓库

Cargo workspace + pnpm workspace。

| 路径 | 职责 |
|---|---|
| `crates/kernel` | 无领域依赖的 Scope / Registry / Lifecycle / BootPlan |
| `crates/plugin-api` | 插件契约：ActionKey、Grant、Manifest |
| `crates/domain-runtime` | Sealed Ingestion / Command / Query 与静态插件组装 |
| `crates/core` | Content / Action / Device / Policy 域模型 |
| `crates/crypto` | Hash、E2EE、Key wrap、dedup HMAC；同一实现供 Desktop 与未来 WASM |
| `crates/storage` | SQLite WAL + Single Writer Queue + 本地 Blob |
| `crates/platform` | 路径、网络变化、前台进程（Best Effort） |
| `crates/clipboard` | 系统剪贴板读写、Watcher、自写去重、敏感标志 |
| `crates/capture` | CaptureBackend / Overlay / Scroll（Phase 4+） |
| `crates/media` | FrameStream / GIF / Video（Phase 5） |
| `crates/sync` | 协议、Router、LAN、Hub transport |
| `hub` | `asterism-hub` 单二进制 |
| `desktop` | Tauri 2 + React |
| `web` | Hub 内嵌的 Web 历史中心 |
| `packages/*` | 共享 TS 包 |
| `deploy/systemd` | 正式部署单元，无 Docker 目录 |

## 硬约束（基线第 50 节）

1. Hub 必须可单二进制运行，不把 Docker 当作依赖。
2. 系统能力在 Rust/Native；React 只负责表现与交互。
3. 截图首屏冻结不能依赖 WebView 冷启动。
4. Clipboard 同步必须先 Normalize、Dedup、Policy，再进入 Transport。
5. 通用图片属于 V1，不只支持本软件截图。
6. 文件跨端必须真实传输，并在接收端重建原生文件剪贴板。
7. Remote Hub 默认只见密文；搜索放客户端。
8. SQLite 所有写入统一 Single Writer Queue。
9. LAN 必须能失败并降级 Hub；mDNS 不是唯一发现方式。
10. AI / 远程桌面只预留接口，V1 不引入重型基础设施。

## 实现约定

- 日志禁止记录剪贴板正文、文件内容、密钥、配对码明文。
- 动态 SQL 标识符必须来自白名单；值使用参数绑定。
- 文件相对路径必须做 Path Traversal 防护；不跟随 symlink/junction。
- 敏感剪贴板默认：不同步、不上传、不入历史。
- 应用排除名单标注为 Best Effort，不得承诺识别应用内部模式（如 Chrome Incognito）。
- Windows 剪贴板禁止轮询；macOS 只低频读 `changeCount`。
- 读连接 2–4 条，不为“异步”扩大连接池。

## 阶段

按基线第 42 节推进，不跳过风险验收：

0. POC：Clipboard / Fast Screenshot / DPI / Scroll / Recording / SQLite / LAN-Hub
1. Core + 本地 Clipboard + 本地 History UI
2. LAN Direct
3. Hub + Web + E2EE
4. Screenshot
5. Scroll / GIF / Video / Audio
6. Hardening

当前目标默认是 **Phase 1**，除非任务明确要求进入后续阶段。

## 验证

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

改动 Desktop / Web UI 时，按全局偏好做端到端验证。无法启动 GUI 时说明未验证项。

Windows 专用路径在 macOS 上只能保证 `cfg` 隔离编译结构，不得声称已在 Windows 实测。

## Git

本地仓库，保持小步可回滚的 commit。每个 commit 只包含一个逻辑单元；不要把无关格式化、依赖升级和功能混在一起。
