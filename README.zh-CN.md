# Asterism

[English](README.md) · [简体中文](README.zh-CN.md)

在 Windows 与 macOS 之间同步剪贴板、截图和文件。自托管一个很小的 Hub，即可远程同步，并打开网页版历史。

## 功能

- Windows 与 macOS 之间同步 **文字**、**图片**、**文件 / 文件夹**
- 优先 **局域网直连**（加密）。局域网不通时走你自己的 Hub
- 远程内容 **端到端加密**。Hub 只存密文，搜索在本机完成
- **敏感剪贴板**（密码管理器、系统「不要同步」标志）默认留在本机
- **截图**：区域、窗口、全屏、标注、滚动长图
- **GIF** 与 **视频** 录制
- Hub 上的 **网页历史**：浏览、搜索、复制或下载。浏览器不监听系统剪贴板

```text
Windows  ◄── 局域网 ──►  macOS
    │                     │
    └────── 你的 Hub ──────┘
               │
           网页历史
```

## 使用

需要 Rust（stable）以及 Node 22+ 和 pnpm。

### 1. 启动 Hub

在 Linux 上，或先在自己的 Mac 上试：

```bash
cargo run -p asterism-hub -- init --data-dir ./data
cargo run -p asterism-hub -- serve --config ./data/config.toml
```

`init` 会打印一次 **bootstrap secret**（同时写入 `data/bootstrap.secret`）。请保存。第一台电脑用这个 secret，之后的电脑用配对码。

Hub 默认监听 `https://127.0.0.1:8787`，证书是自签名的。桌面端第一次连上后会记住这张证书。以后如果换了证书，删掉桌面端 `sync.toml` 里的 `hub_cert_sha256` 再连一次。

Linux 服务器：

```bash
cargo build -p asterism-hub --release
./deploy/install-hub.sh
# 安装 deploy/systemd/asterism-hub.service 后：
# sudo systemctl enable --now asterism-hub
```

### 2. 打开桌面端

```bash
pnpm install
pnpm desktop:dev
```

设置：

1. 填写 Hub URL，例如 `https://127.0.0.1:8787`
2. **第一台电脑**：粘贴 bootstrap secret → 连接并注册本机
3. **下一台电脑**：在第一台上生成配对码，把保险库密钥（AVK）附到配对码上，再用这个码连接

**Recovery Key** 请离线保管，它是保险库的备份。

平常照常复制即可，历史会出现在应用里。截图、滚动长图、GIF、视频用对应的采集操作。

### 3. 打开网页历史

用浏览器打开 Hub 地址（页面由 Hub 提供）。用桌面端生成的配对码配对，再用 Recovery Key 解锁。之后可以搜索、复制文字、下载图片或文件。这个页面不会读取系统剪贴板。

## 许可

[MIT](LICENSE)
