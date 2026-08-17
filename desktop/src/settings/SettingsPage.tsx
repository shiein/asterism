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

interface SyncSettings {
  hub_url: string | null;
  token: string | null;
  lan_port: number;
  auto_sync: boolean;
  auto_receive?: boolean;
  pending_pair_code?: string | null;
  pending_pair_salt?: string | null;
  hub_cert_sha256?: string | null;
}

interface DeviceDto {
  id: string;
  name: string;
  platform: string;
  last_seen_at_ms: number;
  revoked: boolean;
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
      void qc.invalidateQueries();
      success("同步设置已保存");
    },
    onError: (e) => showError(`保存失败：${e}`),
  });

  const connect = useMutation({
    mutationFn: ({ url, pairingCode }: { url: string; pairingCode: string | null }) =>
      invoke<string>("connect_hub", { url, pairingCode }),
    onSuccess: (code) => {
      void qc.invalidateQueries();
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
