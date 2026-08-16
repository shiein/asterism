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
} from "../components/icons";

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

export function SettingsPage() {
  const qc = useQueryClient();
  const { success, error: showError } = useToast();

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
    onError: (e) => showError(`保存失败: ${e}`),
  });

  const connect = useMutation({
    mutationFn: ({ url, pairingCode }: { url: string; pairingCode: string | null }) =>
      invoke<string>("connect_hub", { url, pairingCode }),
    onSuccess: (code) => {
      void qc.invalidateQueries();
      success(`成功连接至 Hub！首次配对码：${code}`);
    },
    onError: (e) => showError(`连接 Hub 失败: ${e}`),
  });

  const pair = useMutation({
    mutationFn: () => invoke<string>("hub_pairing_code"),
    onSuccess: (code) => {
      success(`已生成浏览器配对码: ${code}`);
    },
    onError: (e) => showError(`生成配对码失败: ${e}`),
  });

  const depositAvk = useMutation({
    mutationFn: (code: string) => invoke("publish_pairing_avk", { code }),
    onSuccess: () => success("已成功将端到端密钥存入配对通道"),
    onError: (e) => showError(`存入密钥失败: ${e}`),
  });

  const current = settings.data;
  if (!current) {
    return (
      <main className="main-content">
        <div className="empty-state">
          <div className="empty-state-title">正在读取设置…</div>
        </div>
      </main>
    );
  }

  function handleCopyRecoveryKey() {
    if (!recovery.data) return;
    navigator.clipboard.writeText(recovery.data);
    setCopiedKey(true);
    success("恢复密钥已复制到剪贴板");
    setTimeout(() => setCopiedKey(false), 2000);
  }

  const isConnected = Boolean(current.token && current.hub_url);

  return (
    <main className="main-content">
      <header className="page-header" style={{ padding: "20px 32px" }}>
        <div>
          <h2 style={{ fontSize: 20, fontWeight: 700, letterSpacing: "-0.02em" }}>系统设置</h2>
          <p style={{ fontSize: 13, color: "var(--text-secondary)", marginTop: 2 }}>
            配置端到端加密、Hub 远程中转与局域网节点互联
          </p>
        </div>
      </header>

      <div className="settings-container">
        {/* Hub Sync Section */}
        <section className="settings-section">
          <div className="section-header">
            <div className="section-title">
              <SyncIcon size={18} style={{ color: "var(--accent)" }} />
              <span>Hub 远程中转与 E2EE 密文同步</span>
            </div>
            <div style={{ display: "flex", alignItems: "center", gap: 6, fontSize: 12 }}>
              <span className={`status-dot ${isConnected ? "" : "offline"}`} />
              <span style={{ color: isConnected ? "var(--success)" : "var(--text-muted)" }}>
                {isConnected ? "已建立受信任连接" : "未连接"}
              </span>
            </div>
          </div>

          <div className="form-group">
            <label className="form-label" htmlFor="settings-hub-url">
              Hub 服务器地址 (URL)
            </label>
            <input
              id="settings-hub-url"
              className="form-input"
              defaultValue={current.hub_url ?? ""}
              placeholder="https://hub.yourdomain.com:8787"
              onChange={(e) => setHubUrlInput(e.target.value)}
            />
          </div>

          <div className="form-group">
            <label className="form-label" htmlFor="settings-pair-code">
              配对码 或 首台设备 Bootstrap Secret
            </label>
            <input
              id="settings-pair-code"
              className="form-input"
              placeholder="首台设备填 hub init 输出的 bootstrap secret；后续设备填配对码"
              value={pairingCodeInput}
              onChange={(e) => setPairingCodeInput(e.target.value)}
            />
          </div>

          <div style={{ display: "flex", flexWrap: "wrap", gap: 8, marginTop: 4 }}>
            <button
              className="btn btn-primary"
              disabled={connect.isPending}
              onClick={() => {
                const url = hubUrlInput || current.hub_url || "";
                if (!url) {
                  showError("请先输入 Hub 服务器地址");
                  return;
                }
                connect.mutate({ url, pairingCode: pairingCodeInput.trim() || null });
              }}
            >
              <SyncIcon size={14} />
              <span>{connect.isPending ? "正在连接…" : "连接并注册本机"}</span>
            </button>

            <button
              className="btn btn-secondary"
              disabled={pair.isPending}
              onClick={() => void pair.mutate()}
            >
              <KeyIcon size={14} />
              <span>生成浏览器配对码</span>
            </button>

            {pair.data && (
              <button
                className="btn btn-secondary"
                disabled={depositAvk.isPending}
                onClick={() => depositAvk.mutate(pair.data)}
              >
                <ShieldCheckIcon size={14} />
                <span>将 AVK 注入配对通道</span>
              </button>
            )}

            <button
              className={`btn ${current.auto_sync ? "btn-secondary" : "btn-primary"}`}
              onClick={() => {
                const nextSync = !current.auto_sync;
                save.mutate({
                  ...current,
                  auto_sync: nextSync,
                  hub_url: hubUrlInput || current.hub_url,
                });
              }}
            >
              <span>{current.auto_sync ? "暂停自动同步" : "开启自动同步"}</span>
            </button>
          </div>

          {pair.data && (
            <div
              style={{
                padding: "10px 14px",
                background: "rgba(99, 102, 241, 0.1)",
                borderRadius: "var(--radius-md)",
                border: "1px solid var(--accent-border)",
                fontSize: 13,
                color: "#c7d2fe",
              }}
            >
              浏览器配对码：<strong>{pair.data}</strong>（在 Web 历史中心输入此码完成配对）
            </div>
          )}
        </section>

        {/* Vault & Security Section */}
        <section className="settings-section">
          <div className="section-header">
            <div className="section-title">
              <ShieldCheckIcon size={18} style={{ color: "var(--success)" }} />
              <span>端到端加密与保险库 (E2EE Vault)</span>
            </div>
          </div>

          <div className="form-group">
            <label className="form-label">Recovery Key（恢复密钥，请妥善离线保存）</label>
            <div style={{ display: "flex", gap: 8 }}>
              <input className="form-input" readOnly value={recovery.data ?? ""} />
              <button className="btn btn-secondary" onClick={handleCopyRecoveryKey}>
                {copiedKey ? <CheckIcon size={14} /> : <CopyIcon size={14} />}
                <span>{copiedKey ? "已复制" : "复制密钥"}</span>
              </button>
            </div>
          </div>

          <div style={{ fontSize: 12.5, color: "var(--text-muted)", lineHeight: 1.6 }}>
            Asterism 采用双层密钥派生体系，所有剪贴板正文在离开设备前均由本地主密钥（AVK）高强度加密。云端 Hub 仅保存不可读密文。
          </div>
        </section>

        {/* Connected Devices Section */}
        <section className="settings-section">
          <div className="section-header">
            <div className="section-title">
              <LaptopIcon size={18} style={{ color: "var(--text-secondary)" }} />
              <span>已受信任的互联设备</span>
            </div>
          </div>

          {devices.isLoading && <p style={{ fontSize: 13, color: "var(--text-muted)" }}>正在查询设备列表…</p>}

          {devices.isError && (
            <p style={{ fontSize: 13, color: "var(--text-muted)" }}>未连接到 Hub，仅显示本机设备。</p>
          )}

          {devices.data && devices.data.length > 0 ? (
            <div style={{ display: "grid", gap: 8 }}>
              {devices.data.map((d) => (
                <div
                  key={d.id}
                  style={{
                    display: "flex",
                    alignItems: "center",
                    justifyContent: "space-between",
                    padding: "10px 14px",
                    background: "rgba(0, 0, 0, 0.25)",
                    border: "1px solid var(--border-subtle)",
                    borderRadius: "var(--radius-md)",
                  }}
                >
                  <div>
                    <div style={{ fontWeight: 600, color: "var(--text-primary)" }}>{d.name}</div>
                    <div style={{ fontSize: 11.5, color: "var(--text-muted)" }}>
                      平台：{d.platform} {d.revoked && <span style={{ color: "var(--danger)" }}>（已撤销）</span>}
                    </div>
                  </div>
                  <span className="kind-badge text">{d.platform}</span>
                </div>
              ))}
            </div>
          ) : (
            <div style={{ fontSize: 13, color: "var(--text-muted)" }}>暂无其他配对设备。</div>
          )}
        </section>

        {/* Preferences Section */}
        <section className="settings-section">
          <div className="section-header">
            <div className="section-title">
              <SettingsIcon size={18} style={{ color: "var(--text-secondary)" }} />
              <span>系统偏好</span>
            </div>
          </div>

          <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}>
            <div>
              <div style={{ fontWeight: 600, color: "var(--text-primary)" }}>开机自启动</div>
              <div style={{ fontSize: 12, color: "var(--text-muted)" }}>系统登录后自动在后台保持剪贴板监听与同步服务</div>
            </div>
            <button
              className="btn btn-secondary"
              onClick={async () => {
                try {
                  await invoke("enable_autostart");
                  success("已启用开机自启服务");
                } catch (e) {
                  showError(`配置自启失败: ${e}`);
                }
              }}
            >
              <span>配置自启</span>
            </button>
          </div>
        </section>
      </div>
    </main>
  );
}
