import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";

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
  const settings = useQuery({
    queryKey: ["sync-settings"],
    queryFn: () => invoke<SyncSettings>("get_sync_settings"),
  });
  const recovery = useQuery({ queryKey: ["recovery"], queryFn: () => invoke<string>("recovery_key") });
  const devices = useQuery({
    queryKey: ["hub-devices"],
    queryFn: () => invoke<DeviceDto[]>("hub_devices"),
    retry: false,
  });

  const save = useMutation({
    mutationFn: (s: SyncSettings) => invoke("save_sync_settings", { settings: s }),
    onSuccess: () => qc.invalidateQueries(),
  });
  const connect = useMutation({
    mutationFn: ({ url, pairingCode }: { url: string; pairingCode: string | null }) =>
      invoke<string>("connect_hub", { url, pairingCode }),
    onSuccess: () => qc.invalidateQueries(),
  });
  const pair = useMutation({ mutationFn: () => invoke<string>("hub_pairing_code") });

  const current = settings.data;
  if (!current) return <p className="empty">读取设置…</p>;

  return (
    <section className="card">
      <h2>同步</h2>
      <label className="search">
        Hub URL
        <input
          defaultValue={current.hub_url ?? ""}
          id="hub-url"
          placeholder="https://hub.example:8787"
        />
      </label>
      <label className="search">
        配对码或首台设备 Bootstrap Secret
        <input id="desktop-pairing-code" placeholder="首台设备填 hub init 打印的 secret；后续设备填配对码" />
      </label>
      <div className="actions">
        <button
          onClick={() => {
            const url = (document.getElementById("hub-url") as HTMLInputElement).value;
            const pairingCode = (
              document.getElementById("desktop-pairing-code") as HTMLInputElement
            ).value.trim();
            void connect.mutate({ url, pairingCode: pairingCode || null });
          }}
        >
          连接并注册本机
        </button>
        <button onClick={() => void pair.mutate()}>生成浏览器配对码</button>
        <button
          onClick={() => {
            const code = pair.data;
            if (code) void invoke("publish_pairing_avk", { code });
          }}
        >
          把 AVK 附到配对码
        </button>
        <button onClick={() => void invoke("enable_autostart")}>开机启动</button>
      </div>
      {connect.data && <p className="sub">已连接。首次配对码：{connect.data}</p>}
      {pair.data && <p className="sub">浏览器配对码：{pair.data}</p>}
      <p className="sub">Token：{current.token ? "已保存" : "未登录"} · 端口 {current.lan_port}</p>
      <label className="search">
        Recovery Key（妥善保管）
        <input readOnly value={recovery.data ?? ""} />
      </label>
      <h3>设备</h3>
      <ul className="list">
        {devices.data?.map((d) => (
          <li key={d.id} className="card">
            {d.name} · {d.platform} {d.revoked ? "· 已撤销" : ""}
          </li>
        ))}
      </ul>
      <div className="actions">
        <button
          onClick={() =>
            save.mutate({
              ...current,
              auto_sync: !current.auto_sync,
              hub_url: (document.getElementById("hub-url") as HTMLInputElement).value || current.hub_url,
            })
          }
        >
          {current.auto_sync ? "关闭自动同步" : "打开自动同步"}
        </button>
      </div>
    </section>
  );
}
