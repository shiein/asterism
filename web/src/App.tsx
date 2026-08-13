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

function mergeHistory(cached: HistoryItem[], incoming: HistoryItem[]): HistoryItem[] {
  const byId = new Map(cached.map((it) => [it.id, it]));
  for (const it of incoming) byId.set(it.id, it);
  return [...byId.values()].sort((a, b) => a.created_at_ms - b.created_at_ms || a.id.localeCompare(b.id));
}

export function App() {
  const [base, setBase] = useState(window.location.origin);
  const [code, setCode] = useState("");
  const [recovery, setRecovery] = useState(loadUnlock() ?? "");
  const [token, setToken] = useState(localStorage.getItem("asterism.token") ?? "");
  const [items, setItems] = useState<HistoryItem[]>([]);
  const [previews, setPreviews] = useState<Record<string, string>>({});
  const [query, setQuery] = useState("");
  const [kindFilter, setKindFilter] = useState("");
  const [indexed, setIndexed] = useState(0);
  const [decryptFailed, setDecryptFailed] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [vaultEpoch, setVaultEpoch] = useState(0);

  const api = useMemo(() => new HubApi(base, token || undefined), [base, token]);
  const index = useMemo(() => new LocalIndex(), []);

  useEffect(() => {
    if (!token) return;
    let cancelled = false;
    (async () => {
      try {
        const cached = (await loadShard<HistoryItem[]>("history")) ?? [];
        const cursor = await loadShard<string>("cursor");
        const page = await api.history(200, cursor);
        const list = mergeHistory(cached, page);
        if (cancelled) return;
        setItems([...list].reverse());
        await saveShard("history", list);
        const last = list.at(-1);
        if (last) await saveShard("cursor", `${last.created_at_ms}:${last.id}`);

        index.clear();
        const hex = loadUnlock();
        let failed = 0;
        const nextPreviews: Record<string, string> = {};
        if (!hex || !vaultReady()) {
          for (const it of list) index.add(it.id, it.kind);
          setIndexed(index.size);
          setDecryptFailed(0);
          setPreviews({});
          return;
        }
        for (const it of list) {
          let text = it.kind;
          if (it.encrypted_metadata) {
            try {
              const dec = await decryptPackage(hex, it.encrypted_metadata);
              text += " " + dec.meta;
              nextPreviews[it.id] = dec.meta;
            } catch (err) {
              failed += 1;
              console.warn("decrypt failed", it.id, err);
            }
          }
          index.add(it.id, text);
        }
        if (cancelled) return;
        setIndexed(index.size);
        setDecryptFailed(failed);
        setPreviews(nextPreviews);
      } catch (e) {
        if (!cancelled) setError(String(e));
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [api, token, index, vaultEpoch]);

  const hits = query.trim() ? new Set(index.search(query)) : null;
  const visible = items.filter((it) => {
    if (kindFilter && it.kind !== kindFilter) return false;
    if (!hits) return true;
    return hits.has(it.id) || it.kind.toLowerCase().includes(query.toLowerCase());
  });

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
    else {
      setError(null);
      setVaultEpoch((n) => n + 1);
    }
  }

  async function copyItem(it: HistoryItem) {
    const text = previews[it.id];
    if (!text) {
      setError("该条目尚未解密，无法复制正文");
      return;
    }
    try {
      await navigator.clipboard.writeText(text);
    } catch (e) {
      setError(String(e));
    }
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
            setPreviews({});
            void clearIndex();
          }}
        >
          退出
        </button>
      </section>

      {error && <p className="error">{error}</p>}
      <p className="muted">
        已建立 {indexed} / {items.length} 条历史索引
        {decryptFailed > 0 ? ` · ${decryptFailed} 条解密失败` : ""}
      </p>
      <input className="search" value={query} onChange={(e) => setQuery(e.target.value)} placeholder="搜索已索引历史" />
      <select value={kindFilter} onChange={(e) => setKindFilter(e.target.value)}>
        <option value="">全部类型</option>
        <option value="text">text</option>
        <option value="image">image</option>
        <option value="screenshot">screenshot</option>
        <option value="files">files</option>
        <option value="gif">gif</option>
        <option value="video">video</option>
      </select>

      <ul className="list">
        {visible.map((it) => (
          <li key={it.id}>
            <strong>{it.kind}</strong>
            <span>{new Date(it.created_at_ms).toLocaleString()}</span>
            <span>{it.logical_size} B</span>
            {previews[it.id] && <p className="muted">{previews[it.id].slice(0, 160)}</p>}
            <button onClick={() => void copyItem(it)}>复制</button>
            <button onClick={() => api.deleteHistory(it.id).then(() => setItems((xs) => xs.filter((x) => x.id !== it.id)))}>
              删除
            </button>
          </li>
        ))}
      </ul>
    </main>
  );
}
