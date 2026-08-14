import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect } from "react";
import { listen } from "@tauri-apps/api/event";
import { captureFullscreen, captureRegion, copyItem, deleteItem, getIdentity, listHistory, setFavorite } from "../api";
import { useUiStore } from "../store";
import type { ContentKind, HistoryItem } from "../types";

const KINDS: Array<ContentKind | "ALL"> = ["ALL", "TEXT", "IMAGE", "FILES"];

export function HistoryPage() {
  const queryClient = useQueryClient();
  const { query, kind, favoriteOnly, setQuery, setKind, setFavoriteOnly } = useUiStore();

  const identity = useQuery({ queryKey: ["identity"], queryFn: getIdentity });
  const history = useInfiniteQuery({
    queryKey: ["history", query, kind, favoriteOnly],
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) =>
      listHistory({
        query: query.trim() || undefined,
        kind: kind === "ALL" ? undefined : kind,
        favoriteOnly,
        limit: 80,
        cursor: pageParam,
      }),
    getNextPageParam: (page) => {
      if (page.length < 80) return undefined;
      const last = page.at(-1);
      return last ? `${last.createdAtMs}:${last.id}` : undefined;
    },
  });
  const historyItems = history.data?.pages.flat() ?? [];

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    listen("history-changed", () => {
      void queryClient.invalidateQueries({ queryKey: ["history"] });
    }).then((fn) => {
      unlisten = fn;
    });
    return () => unlisten?.();
  }, [queryClient]);

  const copy = useMutation({ mutationFn: copyItem });
  const fav = useMutation({
    mutationFn: ({ id, favorite }: { id: string; favorite: boolean }) => setFavorite(id, favorite),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["history"] }),
  });
  const remove = useMutation({
    mutationFn: deleteItem,
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["history"] }),
  });

  return (
    <div className="app">
      <header className="top">
        <div>
          <h1>Asterism</h1>
          <p className="sub">
            {identity.data?.deviceName ?? "本机"} · 本地历史 · Phase 1
          </p>
        </div>
        <div className="top-actions">
          <button
            onClick={() => {
              void captureFullscreen().then(() => queryClient.invalidateQueries({ queryKey: ["history"] }));
            }}
          >
            全屏截图
          </button>
          <button
            onClick={() => {
              void captureRegion().then(() => queryClient.invalidateQueries({ queryKey: ["history"] }));
            }}
          >
            选区截图
          </button>
          <label className="search">
            <span>搜索</span>
            <input
              value={query}
              placeholder="文本或文件名"
              onChange={(e) => setQuery(e.target.value)}
            />
          </label>
        </div>
      </header>

      <div className="filters">
        {KINDS.map((k) => (
          <button key={k} className={kind === k ? "chip on" : "chip"} onClick={() => setKind(k)}>
            {labelKind(k)}
          </button>
        ))}
        <button
          className={favoriteOnly ? "chip on" : "chip"}
          onClick={() => setFavoriteOnly(!favoriteOnly)}
        >
          仅收藏
        </button>
      </div>

      {history.isLoading && <p className="empty">正在读取历史…</p>}
      {history.error && <p className="empty error">读取失败：{(history.error as Error).message}</p>}
      {historyItems.length === 0 && !history.isLoading && <p className="empty">还没有可显示的剪贴板历史。</p>}

      <ul className="list">
        {historyItems.map((item) => (
          <li key={item.id} className="card">
            <div className="meta">
              <span className="kind">{labelKind(item.kind)}</span>
              <time>{formatTime(item.createdAtMs)}</time>
              {item.sourceApp && <span className="app">{item.sourceApp}</span>}
            </div>
            <p className="preview">{previewOf(item)}</p>
            <div className="actions">
              <button onClick={() => copy.mutate(item.id)}>复制</button>
              <button onClick={() => fav.mutate({ id: item.id, favorite: !item.favorite })}>
                {item.favorite ? "取消收藏" : "收藏"}
              </button>
              <button className="danger" onClick={() => remove.mutate(item.id)}>
                删除
              </button>
            </div>
          </li>
        ))}
      </ul>
      {history.hasNextPage && (
        <button disabled={history.isFetchingNextPage} onClick={() => void history.fetchNextPage()}>
          {history.isFetchingNextPage ? "正在加载…" : "加载更多"}
        </button>
      )}
    </div>
  );
}

function labelKind(kind: ContentKind | "ALL"): string {
  switch (kind) {
    case "ALL":
      return "全部";
    case "TEXT":
      return "文本";
    case "IMAGE":
      return "图片";
    case "FILES":
      return "文件";
    case "SCREENSHOT":
      return "截图";
    case "GIF":
      return "GIF";
    case "VIDEO":
      return "视频";
  }
}

function previewOf(item: HistoryItem): string {
  if (item.preview) return item.preview;
  if (item.kind === "IMAGE" && item.imageWidth && item.imageHeight) {
    return `${item.imageWidth}×${item.imageHeight} PNG`;
  }
  if (item.kind === "FILES" && item.fileCount != null) {
    return `${item.fileCount} 个文件 · ${formatBytes(item.logicalSize)}`;
  }
  return formatBytes(item.logicalSize);
}

function formatTime(ms: number): string {
  return new Date(ms).toLocaleString();
}

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}
