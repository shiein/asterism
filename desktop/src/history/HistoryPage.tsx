import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  captureFullscreen,
  captureRegion,
  copyItem,
  deleteItem,
  listActions,
  listHistory,
  previewImage,
  setFavorite,
} from "../api";
import { useUiStore } from "../store";
import { useToast } from "../components/Toast";
import { HistoryDetailModal } from "./HistoryDetailModal";
import {
  SearchIcon,
  XIcon,
  CopyIcon,
  StarIcon,
  TrashIcon,
  CropIcon,
  MaximizeIcon,
  ClipboardIcon,
  FolderIcon,
  EditIcon,
  CheckIcon,
} from "../components/icons";
import type { ContentKind, HistoryItem } from "../types";

const PAGE_SIZE = 80;

const KINDS: Array<{ key: ContentKind | "ALL"; label: string }> = [
  { key: "ALL", label: "全部" },
  { key: "TEXT", label: "文本" },
  { key: "IMAGE", label: "图片" },
  { key: "SCREENSHOT", label: "截图" },
  { key: "FILES", label: "文件" },
  { key: "GIF", label: "GIF" },
  { key: "VIDEO", label: "视频" },
];

const KIND_BADGE: Record<ContentKind, { label: string; tone: string }> = {
  TEXT: { label: "文本", tone: "" },
  IMAGE: { label: "图片", tone: "accent" },
  SCREENSHOT: { label: "截图", tone: "success" },
  FILES: { label: "文件", tone: "warning" },
  GIF: { label: "GIF", tone: "info" },
  VIDEO: { label: "视频", tone: "info" },
};

export function HistoryPage({
  onAnnotate,
  favoriteFilter = false,
}: {
  onAnnotate: (id: string) => void;
  favoriteFilter?: boolean;
}) {
  const queryClient = useQueryClient();
  const { toast, success, error: showError } = useToast();
  const { query, kind, favoriteOnly, setQuery, setKind, setFavoriteOnly } = useUiStore();
  const searchInputRef = useRef<HTMLInputElement>(null);

  const [detailItem, setDetailItem] = useState<HistoryItem | null>(null);
  const [copiedId, setCopiedId] = useState<string | null>(null);
  const [capturing, setCapturing] = useState<"region" | "fullscreen" | null>(null);

  const effectiveFavorite = favoriteFilter || favoriteOnly;

  const catalog = useQuery({ queryKey: ["actions"], queryFn: listActions });
  const actionIds = useMemo(
    () => new Set((catalog.data ?? []).map((action) => action.id)),
    [catalog.data]
  );
  const showAction = (id: string) => !catalog.data || actionIds.has(id);

  const history = useInfiniteQuery({
    queryKey: ["history", query, kind, effectiveFavorite],
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) =>
      listHistory({
        query: query.trim() || undefined,
        kind: kind === "ALL" ? undefined : kind,
        favoriteOnly: effectiveFavorite,
        limit: PAGE_SIZE,
        cursor: pageParam,
      }),
    getNextPageParam: (page) => {
      if (page.length < PAGE_SIZE) return undefined;
      const last = page.at(-1);
      return last ? `${last.createdAtMs}:${last.id}` : undefined;
    },
  });

  const historyItems = useMemo(() => history.data?.pages.flat() ?? [], [history.data]);

  useEffect(() => {
    let disposed = false;
    let cleanupFn: (() => void) | undefined;
    listen("history-changed", () => {
      void queryClient.invalidateQueries({ queryKey: ["history"] });
    }).then((fn) => {
      if (disposed) {
        fn();
      } else {
        cleanupFn = fn;
      }
    });
    return () => {
      disposed = true;
      cleanupFn?.();
    };
  }, [queryClient]);

  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      const target = e.target as HTMLElement | null;
      const typing = target?.tagName === "INPUT" || target?.tagName === "TEXTAREA";
      if (e.key === "/" && !typing) {
        e.preventDefault();
        searchInputRef.current?.focus();
      }
    }
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, []);

  const copy = useMutation({
    mutationFn: copyItem,
    onSuccess: (_, id) => {
      setCopiedId(id);
      success("已复制到剪贴板");
      setTimeout(() => setCopiedId(null), 1200);
    },
    onError: (e) => showError(`复制失败：${e}`),
  });

  const fav = useMutation({
    mutationFn: ({ id, favorite }: { id: string; favorite: boolean }) => setFavorite(id, favorite),
    onSuccess: (_, { id, favorite }) => {
      setDetailItem((current) => (current?.id === id ? { ...current, favorite } : current));
      void queryClient.invalidateQueries({ queryKey: ["history"] });
      toast(favorite ? "已加入收藏" : "已取消收藏");
    },
    onError: (e) => showError(`更新收藏失败：${e}`),
  });

  const remove = useMutation({
    mutationFn: deleteItem,
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["history"] });
      toast("已删除该记录");
    },
    onError: (e) => showError(`删除失败：${e}`),
  });

  async function runCapture(mode: "region" | "fullscreen") {
    try {
      setCapturing(mode);
      const id = mode === "region" ? await captureRegion() : await captureFullscreen();
      await queryClient.invalidateQueries({ queryKey: ["history"] });
      success(mode === "region" ? "选区截图已保存并复制" : "全屏截图已保存并复制");
      onAnnotate(id);
    } catch (cause) {
      const detail = String(cause);
      if (!detail.includes("cancelled")) {
        showError(`截图失败：${detail}`);
      }
    } finally {
      setCapturing(null);
    }
  }

  const isEmpty = historyItems.length === 0 && !history.isLoading && !history.error;

  return (
    <main className="pane">
      <header className="pane-header">
        <div className="pane-header-row">
          <div className="search">
            <SearchIcon size={14} />
            <input
              ref={searchInputRef}
              value={query}
              placeholder="搜索文本、文件名或来源应用"
              onChange={(e) => setQuery(e.target.value)}
            />
            <span className="search-trailing">
              {query ? (
                <button
                  className="btn btn-plain btn-icon"
                  style={{ width: 22, height: 22, flex: "0 0 22px" }}
                  onClick={() => setQuery("")}
                  aria-label="清空搜索"
                >
                  <XIcon size={13} />
                </button>
              ) : (
                <kbd className="hint">/</kbd>
              )}
            </span>
          </div>

          <div className="row">
            <button
              className="btn btn-primary"
              disabled={capturing !== null}
              onClick={() => void runCapture("region")}
            >
              <CropIcon size={14} />
              <span>{capturing === "region" ? "正在选区…" : "选区截图"}</span>
            </button>
            <button
              className="btn"
              disabled={capturing !== null}
              onClick={() => void runCapture("fullscreen")}
            >
              <MaximizeIcon size={14} />
              <span>全屏</span>
            </button>
          </div>
        </div>

        <div className="row wrap">
          {KINDS.map((k) => (
            <button
              key={k.key}
              className="chip"
              aria-pressed={kind === k.key}
              onClick={() => setKind(k.key)}
            >
              {k.label}
            </button>
          ))}
          {!favoriteFilter && (
            <button
              className="chip starred"
              aria-pressed={favoriteOnly}
              onClick={() => setFavoriteOnly(!favoriteOnly)}
            >
              <StarIcon size={12} filled={favoriteOnly} />
              仅收藏
            </button>
          )}
        </div>
      </header>

      <div className="pane-body">
        {history.isLoading && (
          <div className="empty">
            <div className="empty-icon">
              <ClipboardIcon size={20} />
            </div>
            <div className="empty-title">正在读取历史…</div>
          </div>
        )}

        {history.error && (
          <div className="notice danger" style={{ marginBottom: 12 }}>
            <XIcon size={15} className="notice-icon" />
            <div>
              <strong>读取历史失败</strong>
              {(history.error as Error).message}
            </div>
          </div>
        )}

        {isEmpty && (
          <div className="empty">
            <div className="empty-icon">
              <SearchIcon size={20} />
            </div>
            <div className="empty-title">
              {query ? "没有匹配的内容" : favoriteFilter ? "还没有收藏" : "还没有剪贴板记录"}
            </div>
            <div className="empty-sub">
              {query
                ? "换个关键词，或清空筛选条件"
                : "复制任意文本、图片或截图，就会出现在这里"}
            </div>
            {query && (
              <button className="btn" onClick={() => setQuery("")}>
                清空搜索
              </button>
            )}
          </div>
        )}

        <div className="history-grid">
          {historyItems.map((item) => (
            <Entry
              key={item.id}
              item={item}
              copied={copiedId === item.id}
              showAction={showAction}
              onCopy={() => copy.mutate(item.id)}
              onFavorite={() => fav.mutate({ id: item.id, favorite: !item.favorite })}
              onDelete={() => remove.mutate(item.id)}
              onAnnotate={() => onAnnotate(item.id)}
              onOpenDetail={() => setDetailItem(item)}
            />
          ))}
        </div>

        {history.hasNextPage && (
          <div style={{ display: "grid", placeItems: "center", marginTop: 18 }}>
            <button
              className="btn"
              disabled={history.isFetchingNextPage}
              onClick={() => void history.fetchNextPage()}
            >
              {history.isFetchingNextPage ? "加载中…" : "加载更多"}
            </button>
          </div>
        )}
      </div>

      <HistoryDetailModal
        item={detailItem}
        isOpen={Boolean(detailItem)}
        onClose={() => setDetailItem(null)}
        canCopy={showAction("asterism.history.copy")}
        canFavorite={showAction("asterism.history.favorite")}
        canDelete={showAction("asterism.history.delete")}
        onCopy={(id) => copy.mutateAsync(id)}
        onFavorite={(id, favState) => fav.mutateAsync({ id, favorite: favState })}
        onDelete={(id) => remove.mutateAsync(id)}
        onAnnotate={(id) => onAnnotate(id)}
      />
    </main>
  );
}

interface EntryProps {
  item: HistoryItem;
  copied: boolean;
  showAction: (id: string) => boolean;
  onCopy: () => void;
  onFavorite: () => void;
  onDelete: () => void;
  onAnnotate: () => void;
  onOpenDetail: () => void;
}

function Entry({
  item,
  copied,
  showAction,
  onCopy,
  onFavorite,
  onDelete,
  onAnnotate,
  onOpenDetail,
}: EntryProps) {
  const isVisual = item.kind === "IMAGE" || item.kind === "SCREENSHOT" || item.kind === "GIF";
  const badge = KIND_BADGE[item.kind];

  return (
    <article className="entry" onClick={onOpenDetail}>
      <div className="entry-head">
        <span className={`badge ${badge.tone}`}>{badge.label}</span>
        {item.sourceApp && <span className="entry-source">{item.sourceApp}</span>}
        <span className="entry-time">{formatRelativeTime(item.createdAtMs)}</span>
        {showAction("asterism.history.favorite") && (
          <button
            className={`star-btn ${item.favorite ? "on" : ""}`}
            onClick={(e) => {
              e.stopPropagation();
              onFavorite();
            }}
            title={item.favorite ? "取消收藏" : "加入收藏"}
          >
            <StarIcon size={14} filled={item.favorite} />
          </button>
        )}
      </div>

      <div className="entry-body">
        {item.kind === "TEXT" && <div className="text-block">{item.preview || "（无预览）"}</div>}

        {isVisual && <Thumbnail id={item.id} />}

        {item.kind === "VIDEO" && (
          <div className="file-block">
            <FolderIcon size={18} style={{ color: "var(--text-tertiary)" }} />
            <div>
              <div className="list-row-title">视频片段</div>
              <div className="list-row-sub">{formatBytes(item.logicalSize)}</div>
            </div>
          </div>
        )}

        {item.kind === "FILES" && (
          <div className="file-block">
            <FolderIcon size={20} style={{ color: "var(--accent)", flexShrink: 0 }} />
            <div style={{ minWidth: 0, flex: 1 }}>
              <div
                className="list-row-title"
                style={{
                  wordBreak: "break-all",
                  whiteSpace: "normal",
                  lineHeight: 1.35,
                  fontSize: 13,
                  fontWeight: 500,
                }}
              >
                {item.preview || "文件"}
              </div>
              <div className="list-row-sub" style={{ marginTop: 2 }}>
                {item.fileCount && item.fileCount > 1
                  ? `${item.fileCount} 个文件 · ${formatBytes(item.logicalSize)}`
                  : formatBytes(item.logicalSize)}
              </div>
            </div>
          </div>
        )}
      </div>

      <div className="entry-foot" onClick={(e) => e.stopPropagation()}>
        <span className="entry-size">{formatBytes(item.logicalSize)}</span>
        <div className="row" style={{ gap: 4 }}>
          {isVisual && (
            <button className="btn btn-plain btn-icon" onClick={onAnnotate} title="标注">
              <EditIcon size={14} />
            </button>
          )}
          {showAction("asterism.history.delete") && (
            <button className="btn btn-plain btn-icon" onClick={onDelete} title="删除">
              <TrashIcon size={14} />
            </button>
          )}
          {showAction("asterism.history.copy") && (
            <button className={copied ? "btn" : "btn btn-primary"} onClick={onCopy}>
              {copied ? <CheckIcon size={13} /> : <CopyIcon size={13} />}
              <span>{copied ? "已复制" : "复制"}</span>
            </button>
          )}
        </div>
      </div>
    </article>
  );
}

function Thumbnail({ id }: { id: string }) {
  const preview = useQuery({
    queryKey: ["preview-image", id],
    queryFn: () => previewImage(id),
    staleTime: 120_000,
  });

  if (preview.isLoading) {
    return <div className="thumb">载入图片…</div>;
  }
  if (preview.error || !preview.data) {
    return (
      <div className="thumb" style={{ color: "var(--danger)" }}>
        缩略图不可用
      </div>
    );
  }
  return (
    <div className="thumb">
      <img src={preview.data} alt="缩略图" loading="lazy" />
    </div>
  );
}

function formatRelativeTime(ms: number): string {
  const seconds = Math.floor((Date.now() - ms) / 1000);
  if (seconds < 45) return "刚刚";
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes} 分钟前`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} 小时前`;
  const days = Math.floor(hours / 24);
  if (days === 1) return "昨天";
  if (days < 7) return `${days} 天前`;
  return new Date(ms).toLocaleDateString();
}

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}
