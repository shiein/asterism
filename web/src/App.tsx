import { useEffect, useMemo, useState } from "react";
import { HubApi, type HistoryItem } from "./api";
import { decryptPackage, loadUnlock, persistUnlock, vaultReady, wipeUnlock } from "./crypto";
import { LocalIndex } from "./search";
import { clearIndex, loadShard, saveShard } from "./search/idb";

const DEVICE_ID_KEY = "asterism.web.device_id";

function deviceId(): string {
  let id = localStorage.getItem(DEVICE_ID_KEY);
  if (!id) {
    id = crypto.randomUUID();
    localStorage.setItem(DEVICE_ID_KEY, id);
  }
  return id;
}

export function App() {
  const [base, setBase] = useState(window.location.origin);
  const [code, setCode] = useState("");
  const [recovery, setRecovery] = useState(loadUnlock() ?? "");
  const [token, setToken] = useState(localStorage.getItem("asterism.token") ?? "");
  const [items, setItems] = useState<HistoryItem[]>([]);
  const [query, setQuery] = useState("");
  const [indexed, setIndexed] = useState(0);
  const [error, setError] = useState<string | null>(null);

  const api = useMemo(() => new HubApi(base, token || undefined), [base, token]);
  const index = useMemo(() => new LocalIndex(), []);

  useEffect(() => {
    if (!token || !vaultReady()) return;
    api
      .history(200)
      .then(async (list) => {
        setItems(list);
        const hex = loadUnlock();
        for (const it of list) {
          let text = `${it.kind}`;
          if (hex && it.encrypted_metadata) {
            try {
              const dec = await decryptPackage(hex, it.encrypted_metadata);
              text += " " + dec.meta;
            } catch {
              /* 密文不可解密时仍可按类型浏览 */
            }
          }
          index.add(it.id, text);
        }
        setIndexed(index.size);
        void saveShard("cursor", list.at(-1)?.created_at_ms ?? 0);
        void saveShard("count", index.size);
      })
      .catch((e) => setError(String(e)));
  }, [api, token, index]);

  const visible = query.trim()
    ? items.filter((it) => index.search(query).includes(it.id) || it.kind.toLowerCase().includes(query.toLowerCase()))
    : items;

  async function pair() {
    setError(null);
    try {
      const session = await api.pairingFinish({
        code: code.trim(),
        device_id: deviceId(),
        device_name: "Browser",
        platform: "browser",
        identity_public_key: [0],
      });
      setToken(session.token);
      localStorage.setItem("asterism.token", session.token);
    } catch (e) {
      setError(String(e));
    }
  }

  function unlock() {
    persistUnlock(recovery);
    if (!vaultReady()) setError("Recovery Key 必须是 64 位 hex");
    else setError(null);
  }

  return (
    <main className="page">
      <h1>Asterism 历史中心</h1>
      <p className="muted">Web 不监听系统剪贴板。Hub 只存密文，搜索在本地索引。</p>

      <section className="panel">
        <label>
          Hub
          <input value={base} onChange={(e) => setBase(e.target.value)} />
        </label>
        <label>
          配对码
          <input value={code} onChange={(e) => setCode(e.target.value)} placeholder="桌面显示的一次性码" />
        </label>
        <button onClick={() => void pair()}>配对</button>
        <label>
          Recovery Key
          <input value={recovery} onChange={(e) => setRecovery(e.target.value)} />
        </label>
        <button onClick={unlock}>解锁 Vault</button>
        <button
          onClick={() => {
            wipeUnlock();
            setToken("");
            localStorage.removeItem("asterism.token");
            setItems([]);
            void clearIndex();
            void loadShard("cursor");
          }}
        >
          退出
        </button>
      </section>

      {error && <p className="error">{error}</p>}
      <p className="muted">已建立 {indexed} / {items.length} 条历史索引</p>
      <input className="search" value={query} onChange={(e) => setQuery(e.target.value)} placeholder="搜索已索引历史" />

      <ul className="list">
        {visible.map((it) => (
          <li key={it.id}>
            <strong>{it.kind}</strong>
            <span>{new Date(it.created_at_ms).toLocaleString()}</span>
            <span>{it.logical_size} B</span>
            <button onClick={() => api.deleteHistory(it.id).then(() => setItems((xs) => xs.filter((x) => x.id !== it.id)))}>
              删除
            </button>
          </li>
        ))}
      </ul>
    </main>
  );
}
