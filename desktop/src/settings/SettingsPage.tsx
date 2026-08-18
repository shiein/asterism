import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { useToast } from "../components/Toast";
import {
  SyncIcon,
  ShieldCheckIcon,
  KeyIcon,
  LaptopIcon,
  CheckIcon,
  CopyIcon,
  SettingsIcon,
  SunIcon,
  MoonIcon,
} from "../components/icons";
import { useUiStore, type AppTheme } from "../store";

interface WebdavConfig {
  enabled: boolean;
  url: string;
  username?: string | null;
  password?: string | null;
}

interface SyncSettings {
  hub_url: string | null;
  token: string | null;
  lan_port: number;
  auto_sync: boolean;
  auto_receive?: boolean;
  pending_pair_code?: string | null;
  pending_pair_salt?: string | null;
  hub_cert_sha256?: string | null;
  webdav?: WebdavConfig | null;
}

interface DeviceDto {
  id: string;
  name: string;
  platform: string;
  last_seen_at_ms: number;
  revoked: boolean;
}

interface LanPeerDto {
  device_id: string;
  name: string;
  addresses: string[];
  port: number;
  fingerprint: string;
  is_trusted: boolean;
}

const THEME_CARDS: Array<{ key: AppTheme; title: string; icon: React.ReactNode; preview: React.ReactNode }> = [
  {
    key: "light",
    title: "浅色",
    icon: <SunIcon size={13} />,
    preview: (
      <div className="theme-preview">
        <i className="light" />
      </div>
    ),
  },
  {
    key: "auto",
    title: "跟随系统",
    icon: <LaptopIcon size={13} />,
    preview: (
      <div className="theme-preview">
        <i className="light" />
        <i className="dark" />
      </div>
    ),
  },
  {
    key: "dark",
    title: "深色",
    icon: <MoonIcon size={13} />,
    preview: (
      <div className="theme-preview">
        <i className="dark" />
      </div>
    ),
  },
];

export function SettingsPage() {
  const qc = useQueryClient();
  const { success, error: showError } = useToast();
  const { theme, setTheme } = useUiStore();

  const [copiedKey, setCopiedKey] = useState(false);
  const [hubUrlInput, setHubUrlInput] = useState("");
  const [pairingCodeInput, setPairingCodeInput] = useState("");

  const settings = useQuery({
    queryKey: ["sync-settings"],
    queryFn: () => invoke<SyncSettings>("get_sync_settings"),
  });

  const recovery = useQuery({
    queryKey: ["recovery"],
    queryFn: () => invoke<string>("recovery_key"),
  });

  const devices = useQuery({
    queryKey: ["hub-devices"],
    queryFn: () => invoke<DeviceDto[]>("hub_devices"),
    retry: false,
  });

  const save = useMutation({
    mutationFn: (s: SyncSettings) => invoke("save_sync_settings", { settings: s }),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ["sync-settings"] });
      success("同步设置已保存");
    },
    onError: (e) => showError(`保存失败：${e}`),
  });

  const connect = useMutation({
    mutationFn: ({ url, pairingCode }: { url: string; pairingCode: string | null }) =>
      invoke<string>("connect_hub", { url, pairingCode }),
    onSuccess: (code) => {
      void qc.invalidateQueries({ queryKey: ["sync-settings"] });
      void qc.invalidateQueries({ queryKey: ["hub-devices"] });
      success(`已连接 Hub，首次配对码：${code}`);
    },
    onError: (e) => showError(`连接 Hub 失败：${e}`),
  });

  const pair = useMutation({
    mutationFn: () => invoke<string>("hub_pairing_code"),
    onSuccess: (code) => success(`浏览器配对码：${code}`),
    onError: (e) => showError(`生成配对码失败：${e}`),
  });

  const depositAvk = useMutation({
    mutationFn: (code: string) => invoke("publish_pairing_avk", { code }),
    onSuccess: () => success("已将端到端密钥存入配对通道"),
    onError: (e) => showError(`存入密钥失败：${e}`),
  });

  const [webdavUrl, setWebdavUrl] = useState("");
  const [webdavUser, setWebdavUser] = useState("");
  const [webdavPass, setWebdavPass] = useState("");

  const testWebdav = useMutation({
    mutationFn: (config: WebdavConfig) => invoke("test_webdav", { config }),
    onSuccess: () => success("WebDAV 连接测试成功！"),
    onError: (e) => showError(`WebDAV 连接失败：${e}`),
  });

  const [copiedFp, setCopiedFp] = useState(false);
  const [manualPeerId, setManualPeerId] = useState("");
  const [manualPeerFp, setManualPeerFp] = useState("");
  const [manualPeerName, setManualPeerName] = useState("");
  const [showManualAdd, setShowManualAdd] = useState(false);

  const localFingerprint = useQuery({
    queryKey: ["local-cert-fingerprint"],
    queryFn: () => invoke<string>("get_local_cert_fingerprint"),
  });

  const lanPeers = useQuery({
    queryKey: ["lan-peers"],
    queryFn: () => invoke<LanPeerDto[]>("get_lan_peers"),
    refetchInterval: 3000,
  });

  const trustPeer = useMutation({
    mutationFn: ({ deviceId, fingerprint, name }: { deviceId: string; fingerprint: string; name: string }) =>
      invoke("trust_lan_peer", { deviceId, fingerprint, name }),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ["lan-peers"] });
      success("已信任该设备，现可直接通过局域网互相同步");
    },
    onError: (e) => showError(`信任设备失败：${e}`),
  });

  const untrustPeer = useMutation({
    mutationFn: (deviceId: string) => invoke("untrust_lan_peer", { deviceId }),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ["lan-peers"] });
      success("已解除信任该设备");
    },
    onError: (e) => showError(`解除信任失败：${e}`),
  });

  const current = settings.data;

  if (settings.isError) {
    return (
      <main className="pane">
        <div className="empty">
          <div className="empty-title" style={{ color: "var(--danger)" }}>
            读取设置失败
          </div>
          <button className="btn" onClick={() => void settings.refetch()}>
            重试
          </button>
        </div>
      </main>
    );
  }

  if (!current) {
    return (
      <main className="pane">
        <div className="empty">
          <div className="empty-title">正在读取设置…</div>
        </div>
      </main>
    );
  }

  async function handleCopyRecoveryKey() {
    if (!recovery.data) return;
    try {
      await invoke("copy_recovery_key");
      setCopiedKey(true);
      success("恢复密钥已复制到剪贴板");
      setTimeout(() => setCopiedKey(false), 2000);
    } catch (e) {
      showError(`复制恢复密钥失败：${e}`);
    }
  }

  const isHubConfigured = Boolean(current.token && current.hub_url);

  return (
    <main className="pane">
      <header className="pane-header">
        <div className="pane-header-row">
          <div>
            <div className="pane-title">设置</div>
            <div className="pane-subtitle">外观、端到端加密、Hub 中转与设备互联</div>
          </div>
        </div>
      </header>

      <div className="pane-body centered">
        <section className="section">
          <div className="section-head">
            <h3>
              <SunIcon size={15} />
              外观
            </h3>
            <span className="badge">
              {theme === "light" ? "浅色" : theme === "dark" ? "深色" : "跟随系统"}
            </span>
          </div>
          <div className="section-body">
            <div className="theme-cards">
              {THEME_CARDS.map((card) => (
                <button
                  key={card.key}
                  className="theme-card"
                  aria-pressed={theme === card.key}
                  onClick={() => setTheme(card.key)}
                >
                  {card.preview}
                  <span className="theme-card-title">
                    {card.icon}
                    {card.title}
                  </span>
                </button>
              ))}
            </div>
          </div>
        </section>

        <section className="section">
          <div className="section-head">
            <h3>
              <SyncIcon size={15} />
              Hub 中转与密文同步
            </h3>
            <span className="row" style={{ gap: 5, fontSize: 12, color: "var(--text-secondary)" }}>
              <span className={`status-dot ${isHubConfigured ? "" : "idle"}`} />
              {isHubConfigured ? "已配置" : "未配置"}
            </span>
          </div>
          <div className="section-body">
            <div className="field">
              <label className="field-label" htmlFor="settings-hub-url">
                Hub 地址
              </label>
              <input
                id="settings-hub-url"
                className="input"
                defaultValue={current.hub_url ?? ""}
                placeholder="https://hub.example.com:8787"
                onChange={(e) => setHubUrlInput(e.target.value)}
              />
            </div>

            <div className="field">
              <label className="field-label" htmlFor="settings-pair-code">
                配对码 / Bootstrap Secret
              </label>
              <input
                id="settings-pair-code"
                className="input"
                placeholder="首台设备填 hub init 输出的 secret，后续设备填配对码"
                value={pairingCodeInput}
                onChange={(e) => setPairingCodeInput(e.target.value)}
              />
            </div>

            <div className="row wrap">
              <button
                className="btn btn-primary"
                disabled={connect.isPending}
                onClick={() => {
                  const url = hubUrlInput || current.hub_url || "";
                  if (!url) {
                    showError("请先填写 Hub 地址");
                    return;
                  }
                  connect.mutate({ url, pairingCode: pairingCodeInput.trim() || null });
                }}
              >
                <SyncIcon size={14} />
                <span>{connect.isPending ? "连接中…" : "连接并注册本机"}</span>
              </button>

              <button className="btn" disabled={pair.isPending} onClick={() => void pair.mutate()}>
                <KeyIcon size={14} />
                <span>生成浏览器配对码</span>
              </button>

              {pair.data && (
                <button
                  className="btn"
                  disabled={depositAvk.isPending}
                  onClick={() => depositAvk.mutate(pair.data)}
                >
                  <ShieldCheckIcon size={14} />
                  <span>注入 AVK</span>
                </button>
              )}

              <button
                className="btn"
                onClick={() =>
                  save.mutate({
                    ...current,
                    auto_sync: !current.auto_sync,
                    hub_url: hubUrlInput || current.hub_url,
                  })
                }
              >
                {current.auto_sync ? "暂停自动同步" : "开启自动同步"}
              </button>
            </div>

            {pair.data && (
              <div className="notice accent">
                <div>
                  浏览器配对码 <strong style={{ display: "inline" }}>{pair.data}</strong>
                  ，在 Web 历史中心输入即可完成配对。
                </div>
              </div>
            )}
          </div>
        </section>

        <section className="section">
          <div className="section-head">
            <h3>
              <SyncIcon size={15} />
              WebDAV 远端存储与同步（Hub 替代方案）
            </h3>
            <span className="row" style={{ gap: 5, fontSize: 12, color: "var(--text-secondary)" }}>
              <span className={`status-dot ${current.webdav?.enabled ? "" : "idle"}`} />
              {current.webdav?.enabled ? "已启用" : "未启用"}
            </span>
          </div>
          <div className="section-body">
            <p className="field-hint" style={{ marginBottom: 12 }}>
              支持使用自建 NAS（群晖/QNAP）、Nextcloud、坚果云或 InfiniCloud 等标准 WebDAV 网盘作为同步介质。数据同样受端到端加密（E2EE）保护。
            </p>

            <div className="field">
              <label className="field-label" htmlFor="settings-webdav-url">
                WebDAV 服务端完整地址
              </label>
              <input
                id="settings-webdav-url"
                className="input mono"
                defaultValue={current.webdav?.url ?? ""}
                placeholder="https://dav.jianguoyun.com/dav/Asterism"
                onChange={(e) => setWebdavUrl(e.target.value)}
              />
            </div>

            <div className="row" style={{ gap: 12 }}>
              <div className="field" style={{ flex: 1 }}>
                <label className="field-label" htmlFor="settings-webdav-user">
                  用户名 / 账号
                </label>
                <input
                  id="settings-webdav-user"
                  className="input"
                  defaultValue={current.webdav?.username ?? ""}
                  placeholder="username@example.com"
                  onChange={(e) => setWebdavUser(e.target.value)}
                />
              </div>

              <div className="field" style={{ flex: 1 }}>
                <label className="field-label" htmlFor="settings-webdav-pass">
                  应用授权密码
                </label>
                <input
                  id="settings-webdav-pass"
                  type="password"
                  className="input"
                  defaultValue={current.webdav?.password ?? ""}
                  placeholder="••••••••••••"
                  onChange={(e) => setWebdavPass(e.target.value)}
                />
              </div>
            </div>

            <div className="row wrap" style={{ marginTop: 10 }}>
              <button
                className="btn"
                disabled={testWebdav.isPending}
                onClick={() => {
                  const url = (webdavUrl !== "" ? webdavUrl : (current.webdav?.url ?? "")).trim();
                  if (!url) {
                    showError("请先填写 WebDAV 地址");
                    return;
                  }
                  testWebdav.mutate({
                    enabled: true,
                    url,
                    username: webdavUser !== "" ? webdavUser.trim() : (current.webdav?.username ?? null),
                    password: webdavPass !== "" ? webdavPass : (current.webdav?.password ?? null),
                  });
                }}
              >
                <ShieldCheckIcon size={14} />
                <span>{testWebdav.isPending ? "测试连接中…" : "测试连接"}</span>
              </button>

              <button
                className="btn btn-primary"
                onClick={() => {
                  const url = (webdavUrl !== "" ? webdavUrl : (current.webdav?.url ?? "")).trim();
                  if (!url) {
                    showError("请先填写 WebDAV 地址");
                    return;
                  }
                  save.mutate({
                    ...current,
                    webdav: {
                      enabled: true,
                      url,
                      username: webdavUser !== "" ? webdavUser.trim() : (current.webdav?.username ?? null),
                      password: webdavPass !== "" ? webdavPass : (current.webdav?.password ?? null),
                    },
                  });
                }}
              >
                保存并启用 WebDAV
              </button>

              {current.webdav?.enabled && (
                <button
                  className="btn"
                  onClick={() => {
                    save.mutate({
                      ...current,
                      webdav: current.webdav ? { ...current.webdav, enabled: false } : null,
                    });
                  }}
                >
                  停用 WebDAV
                </button>
              )}
            </div>
          </div>
        </section>

        <section className="section">
          <div className="section-head">
            <h3>
              <LaptopIcon size={15} />
              局域网直连与设备配对（免 Hub）
            </h3>
            <span className="badge">
              {lanPeers.data ? `发现 ${lanPeers.data.length} 台设备` : "搜索中…"}
            </span>
          </div>
          <div className="section-body">
            <div className="field">
              <label className="field-label">本机 TLS 证书指纹（用于局域网对端验证）</label>
              <div className="row">
                <input
                  className="input mono"
                  readOnly
                  value={localFingerprint.data ?? "加载中…"}
                />
                <button
                  className="btn"
                  onClick={() => {
                    if (localFingerprint.data) {
                      void navigator.clipboard.writeText(localFingerprint.data);
                      setCopiedFp(true);
                      success("本机指纹已复制");
                      setTimeout(() => setCopiedFp(false), 2000);
                    }
                  }}
                >
                  {copiedFp ? <CheckIcon size={14} /> : <CopyIcon size={14} />}
                  <span>{copiedFp ? "已复制" : "复制指纹"}</span>
                </button>
              </div>
              <p className="field-hint">
                局域网内两台设备互相添加对方指纹后，无需经过 Hub 即可直接建立 TLS 加密直连并毫秒级同步剪贴板。
              </p>
            </div>

            <div className="field">
              <label className="field-label">局域网内发现的设备 (mDNS)</label>
              {lanPeers.isLoading && <p className="field-hint">正在搜索局域网对端…</p>}
              {lanPeers.data && lanPeers.data.length === 0 && (
                <p className="field-hint">当前局域网暂未发现其他运行 Asterism 的设备。</p>
              )}
              {lanPeers.data &&
                lanPeers.data.map((peer) => (
                  <div key={peer.device_id} className="list-row" style={{ alignItems: "center" }}>
                    <div>
                      <div className="list-row-title" style={{ display: "flex", alignItems: "center", gap: 8 }}>
                        <span>{peer.name}</span>
                        <span
                          className={`badge ${peer.is_trusted ? "badge-success" : ""}`}
                          style={{
                            fontSize: 11,
                            background: peer.is_trusted ? "rgba(16, 185, 129, 0.15)" : "var(--bg-tertiary)",
                            color: peer.is_trusted ? "var(--accent)" : "var(--text-secondary)",
                          }}
                        >
                          {peer.is_trusted ? "已信任 (直连可用)" : "未信任"}
                        </span>
                      </div>
                      <div className="list-row-sub mono" style={{ fontSize: 11 }}>
                        {peer.addresses.join(", ")} · 指纹: {peer.fingerprint.slice(0, 16)}…
                      </div>
                    </div>
                    <div>
                      {peer.is_trusted ? (
                        <button
                          className="btn"
                          style={{ color: "var(--danger)" }}
                          disabled={untrustPeer.isPending}
                          onClick={() => untrustPeer.mutate(peer.device_id)}
                        >
                          解除信任
                        </button>
                      ) : (
                        <button
                          className="btn btn-primary"
                          disabled={trustPeer.isPending}
                          onClick={() =>
                            trustPeer.mutate({
                              deviceId: peer.device_id,
                              fingerprint: peer.fingerprint,
                              name: peer.name,
                            })
                          }
                        >
                          信任并配对
                        </button>
                      )}
                    </div>
                  </div>
                ))}
            </div>

            <div style={{ marginTop: 12 }}>
              {!showManualAdd ? (
                <button
                  className="btn"
                  onClick={() => setShowManualAdd(true)}
                  style={{ fontSize: 12 }}
                >
                  + 手动添加局域网设备指纹
                </button>
              ) : (
                <div
                  style={{
                    padding: 12,
                    background: "var(--bg-secondary)",
                    borderRadius: "var(--radius-md)",
                    border: "1px solid var(--border-color)",
                    display: "flex",
                    flexDirection: "column",
                    gap: 8,
                  }}
                >
                  <div className="field-label" style={{ fontWeight: 600 }}>
                    手动添加信任设备
                  </div>
                  <input
                    className="input"
                    placeholder="设备名称（如 Windows 台式机）"
                    value={manualPeerName}
                    onChange={(e) => setManualPeerName(e.target.value)}
                  />
                  <input
                    className="input mono"
                    placeholder="设备 ID（UUID）"
                    value={manualPeerId}
                    onChange={(e) => setManualPeerId(e.target.value)}
                  />
                  <input
                    className="input mono"
                    placeholder="TLS 证书指纹 (64位 Hex)"
                    value={manualPeerFp}
                    onChange={(e) => setManualPeerFp(e.target.value)}
                  />
                  <div className="row" style={{ justifyContent: "flex-end", gap: 8 }}>
                    <button className="btn" onClick={() => setShowManualAdd(false)}>
                      取消
                    </button>
                    <button
                      className="btn btn-primary"
                      disabled={!manualPeerId || !manualPeerFp || trustPeer.isPending}
                      onClick={() => {
                        const fp = manualPeerFp.trim();
                        const id = manualPeerId.trim();
                        if (!/^[a-fA-F0-9]{64}$/.test(fp)) {
                          showError("证书指纹必须为 64 位十六进制字符 (SHA-256)");
                          return;
                        }
                        if (!id) {
                          showError("请输入有效的设备 ID");
                          return;
                        }
                        trustPeer.mutate({
                          deviceId: id,
                          fingerprint: fp.toLowerCase(),
                          name: manualPeerName.trim() || "Manual-Peer",
                        });
                        setManualPeerId("");
                        setManualPeerFp("");
                        setManualPeerName("");
                        setShowManualAdd(false);
                      }}
                    >
                      保存并信任
                    </button>
                  </div>
                </div>
              )}
            </div>
          </div>
        </section>

        <section className="section">
          <div className="section-head">
            <h3>
              <ShieldCheckIcon size={15} />
              端到端加密保险库
            </h3>
          </div>
          <div className="section-body">
            <div className="field">
              <label className="field-label">恢复密钥（请离线保存）</label>
              <div className="row">
                <input className="input mono" readOnly value={recovery.data ?? ""} />
                <button className="btn" onClick={() => void handleCopyRecoveryKey()}>
                  {copiedKey ? <CheckIcon size={14} /> : <CopyIcon size={14} />}
                  <span>{copiedKey ? "已复制" : "复制"}</span>
                </button>
              </div>
              <p className="field-hint">
                剪贴板正文在离开设备前由本地主密钥（AVK）加密，Hub 只保存不可读密文。
              </p>
            </div>
          </div>
        </section>

        <section className="section">
          <div className="section-head">
            <h3>
              <LaptopIcon size={15} />
              互联设备
            </h3>
          </div>
          <div className="section-body">
            {devices.isLoading && <p className="field-hint">正在查询设备列表…</p>}
            {devices.isError && <p className="field-hint">未连接 Hub，仅有本机设备。</p>}
            {devices.data && devices.data.length > 0
              ? devices.data.map((d) => (
                  <div key={d.id} className="list-row">
                    <div>
                      <div className="list-row-title">{d.name}</div>
                      <div className="list-row-sub">
                        {d.platform}
                        {d.revoked && <span style={{ color: "var(--danger)" }}> · 已撤销</span>}
                      </div>
                    </div>
                    <span className="badge">{d.platform}</span>
                  </div>
                ))
              : !devices.isLoading && !devices.isError && (
                  <p className="field-hint">暂无其他配对设备。</p>
                )}
          </div>
        </section>

        <section className="section">
          <div className="section-head">
            <h3>
              <SettingsIcon size={15} />
              系统偏好
            </h3>
          </div>
          <div className="section-body">
            <div className="list-row">
              <div>
                <div className="list-row-title">开机自启动</div>
                <div className="list-row-sub">登录后在后台保持剪贴板监听与同步</div>
              </div>
              <button
                className="btn"
                onClick={async () => {
                  try {
                    await invoke("enable_autostart");
                    success("已配置开机自启");
                  } catch (e) {
                    showError(`配置自启失败：${e}`);
                  }
                }}
              >
                配置
              </button>
            </div>
          </div>
        </section>
      </div>
    </main>
  );
}
