import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useRef, useState, useMemo } from "react";
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
  CameraIcon,
  EditIcon,
  FolderIcon,
  CheckIcon,
} from "../components/icons";
import type { ContentKind, HistoryItem } from "../types";

const KINDS: Array<{ key: ContentKind | "ALL"; label: string }> = [
  { key: "ALL", label: "全部" },
  { key: "TEXT", label: "文本" },
  { key: "IMAGE", label: "图片" },
  { key: "SCREENSHOT", label: "截图" },
  { key: "FILES", label: "文件" },
  { key: "GIF", label: "GIF" },
  { key: "VIDEO", label: "视频" },
];

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
        limit: 80,
        cursor: pageParam,
      }),
    getNextPageParam: (page) => {
      if (page.length < 80) return undefined;
      const last = page.at(-1);
      return last ? `${last.createdAtMs}:${last.id}` : undefined;
    },
  });

  const historyItems = useMemo(() => history.data?.pages.flat() ?? [], [history.data]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    listen("history-changed", () => {
      void queryClient.invalidateQueries({ queryKey: ["history"] });
    }).then((fn) => {
      unlisten = fn;
    });
    return () => unlisten?.();
  }, [queryClient]);

  // Global hotkey to focus search bar with '/'
  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      if (e.key === "/" && document.activeElement !== searchInputRef.current) {
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
    onError: (e) => showError(`复制失败: ${e}`),
  });

  const fav = useMutation({
    mutationFn: ({ id, favorite }: { id: string; favorite: boolean }) => setFavorite(id, favorite),
    onSuccess: (_, { id, favorite }) => {
      setDetailItem((current) =>
        current?.id === id ? { ...current, favorite } : current
      );
      void queryClient.invalidateQueries({ queryKey: ["history"] });
      toast(favorite ? "已添加至收藏" : "已从收藏移除");
    },
    onError: (e) => showError(`更新收藏失败: ${e}`),
  });

  const remove = useMutation({
    mutationFn: deleteItem,
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["history"] });
      toast("已删除剪贴板记录");
    },
    onError: (e) => showError(`删除失败: ${e}`),
  });

  function handleOpenDetail(item: HistoryItem) {
    setDetailItem(item);
  }

  return (
    <main className="main-content">
      {/* Top Command & Search Bar */}
      <header className="page-header">
        <div className="header-top">
          <div className="search-container">
            <SearchIcon size={16} className="search-icon" />
            <input
              ref={searchInputRef}
              className="search-input"
              value={query}
              placeholder="搜索剪贴板文本、文件名、应用..."
              onChange={(e) => setQuery(e.target.value)}
            />
            {query ? (
              <button
                className="btn btn-ghost btn-icon search-shortcut"
                style={{ right: 8, height: 24, width: 24 }}
                onClick={() => setQuery("")}
                aria-label="清空搜索"
              >
                <XIcon size={14} />
              </button>
            ) : (
              <span className="search-shortcut">/</span>
            )}
          </div>

          <div className="header-actions">
            <button
              className="btn btn-primary"
              onClick={() => {
                void captureRegion().then((id) => {
                  void queryClient.invalidateQueries({ queryKey: ["history"] });
                  success("选区截图已生成");
                  onAnnotate(id);
                });
              }}
            >
              <CropIcon size={15} />
              <span>选区截图</span>
            </button>
            <button
              className="btn btn-secondary"
              onClick={() => {
                void captureFullscreen().then((id) => {
                  void queryClient.invalidateQueries({ queryKey: ["history"] });
                  success("全屏截图已生成");
                  onAnnotate(id);
                });
              }}
            >
              <MaximizeIcon size={15} />
              <span>全屏截图</span>
            </button>
          </div>
        </div>

        {/* Filter Pills */}
        <div className="filter-bar">
          {KINDS.map((k) => (
            <button
              key={k.key}
              className={`filter-chip ${kind === k.key ? "active" : ""}`}
              onClick={() => setKind(k.key)}
            >
              <span>{k.label}</span>
            </button>
          ))}
          {!favoriteFilter && (
            <button
              className={`filter-chip ${favoriteOnly ? "active-fav" : ""}`}
              onClick={() => setFavoriteOnly(!favoriteOnly)}
            >
              <StarIcon size={13} filled={favoriteOnly} />
              <span>仅看收藏</span>
            </button>
          )}
        </div>
      </header>

      {/* History Grid / Feed */}
      <div className="history-feed">
        {history.isLoading && (
          <div className="empty-state">
            <div className="empty-state-icon">
              <CameraIcon size={24} />
            </div>
            <div className="empty-state-title">正在读取剪贴板历史…</div>
          </div>
        )}

        {history.error && (
          <div className="empty-state">
            <div className="empty-state-title" style={{ color: "var(--danger)" }}>
              读取失败：{(history.error as Error).message}
            </div>
          </div>
        )}

        {historyItems.length === 0 && !history.isLoading && !history.error && (
          <div className="empty-state">
            <div className="empty-state-icon">
              <SearchIcon size={24} />
            </div>
            <div className="empty-state-title">
              {query ? "未找到匹配的剪贴板内容" : "还没有可显示的剪贴板历史"}
            </div>
            <div className="empty-state-sub">
              {query ? "尝试更换搜索词，或清空筛选条件" : "复制任何文本、图片或截图，即可在此即时记录与同步"}
            </div>
            {query && (
              <button className="btn btn-secondary" onClick={() => setQuery("")} style={{ marginTop: 8 }}>
                清空搜索
              </button>
            )}
          </div>
        )}

        <div className="history-grid">
          {historyItems.map((item) => (
            <HistoryCard
              key={item.id}
              item={item}
              copied={copiedId === item.id}
              showAction={showAction}
              onCopy={() => copy.mutate(item.id)}
              onFavorite={() => fav.mutate({ id: item.id, favorite: !item.favorite })}
              onDelete={() => remove.mutate(item.id)}
              onAnnotate={() => onAnnotate(item.id)}
              onOpenDetail={() => handleOpenDetail(item)}
            />
          ))}
        </div>

        {history.hasNextPage && (
          <div style={{ textAlign: "center", marginTop: 24 }}>
            <button
              className="btn btn-secondary"
              disabled={history.isFetchingNextPage}
              onClick={() => void history.fetchNextPage()}
            >
              {history.isFetchingNextPage ? "正在加载更多…" : "加载更多历史"}
            </button>
          </div>
        )}
      </div>

      {/* History Detail Modal */}
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

interface HistoryCardProps {
  item: HistoryItem;
  copied: boolean;
  showAction: (id: string) => boolean;
  onCopy: () => void;
  onFavorite: () => void;
  onDelete: () => void;
  onAnnotate: () => void;
  onOpenDetail: () => void;
}

function HistoryCard({
  item,
  copied,
  showAction,
  onCopy,
  onFavorite,
  onDelete,
  onAnnotate,
  onOpenDetail,
}: HistoryCardProps) {
  const isVisual = item.kind === "IMAGE" || item.kind === "SCREENSHOT" || item.kind === "GIF";

  return (
    <div className="history-card" onClick={onOpenDetail}>
      {/* Header Meta */}
      <div className="card-header">
        <div className="card-meta-left">
          <span className={`kind-badge ${item.kind.toLowerCase()}`}>{kindLabel(item.kind)}</span>
          {item.sourceApp && <span className="source-app-tag">{item.sourceApp}</span>}
          <span className="card-time">{formatRelativeTime(item.createdAtMs)}</span>
        </div>
        <div className="card-header-actions" onClick={(e) => e.stopPropagation()}>
          {showAction("asterism.history.favorite") && (
            <button
              className={`fav-btn ${item.favorite ? "is-fav" : ""}`}
              onClick={onFavorite}
              title={item.favorite ? "取消收藏" : "加入收藏"}
            >
              <StarIcon size={15} filled={item.favorite} />
            </button>
          )}
        </div>
      </div>

      {/* Body Content */}
      <div className="card-body">
        {item.kind === "TEXT" && (
          <div className="text-preview-box">{item.preview || "无预览文本"}</div>
        )}

        {isVisual && (
          <CardImageThumbnail id={item.id} />
        )}

        {item.kind === "FILES" && (
          <div className="files-preview-box">
            <FolderIcon size={22} style={{ color: "var(--warning)" }} />
            <div>
              <div className="files-info-title">{item.preview || "文件集合"}</div>
              <div className="files-info-sub">
                {item.fileCount ?? 1} 个文件 · {formatBytes(item.logicalSize)}
              </div>
            </div>
          </div>
        )}
      </div>

      {/* Footer Actions */}
      <div className="card-footer" onClick={(e) => e.stopPropagation()}>
        <span className="card-size-label">{formatBytes(item.logicalSize)}</span>
        <div className="card-action-btns">
          {isVisual && (
            <button className="btn btn-ghost btn-icon" onClick={onAnnotate} title="标注">
              <EditIcon size={14} />
            </button>
          )}
          {showAction("asterism.history.delete") && (
            <button
              className="btn btn-ghost btn-icon"
              style={{ color: "var(--text-muted)" }}
              onClick={onDelete}
              title="删除"
            >
              <TrashIcon size={14} />
            </button>
          )}
          {showAction("asterism.history.copy") && (
            <button
              className={`btn ${copied ? "btn-secondary" : "btn-primary"}`}
              style={{ padding: "4px 10px", fontSize: 12 }}
              onClick={onCopy}
            >
              {copied ? <CheckIcon size={13} /> : <CopyIcon size={13} />}
              <span>{copied ? "已复制" : "复制"}</span>
            </button>
          )}
        </div>
      </div>
    </div>
  );
}

function CardImageThumbnail({
  id,
}: {
  id: string;
}) {
  const preview = useQuery({
    queryKey: ["preview-image", id],
    queryFn: () => previewImage(id),
    staleTime: 120_000,
  });

  if (preview.isLoading) {
    return (
      <div className="thumbnail-preview-container" style={{ height: 110 }}>
        <span style={{ fontSize: 12, color: "var(--text-muted)" }}>载入图片中…</span>
      </div>
    );
  }

  if (preview.error || !preview.data) {
    return (
      <div className="thumbnail-preview-container" style={{ height: 90 }}>
        <span style={{ fontSize: 12, color: "var(--danger)" }}>缩略图无法展示</span>
      </div>
    );
  }

  return (
    <div className="thumbnail-preview-container">
      <img className="card-thumbnail" src={preview.data} alt="缩略图" loading="lazy" />
    </div>
  );
}

function kindLabel(kind: ContentKind): string {
  switch (kind) {
    case "TEXT": return "文本";
    case "IMAGE": return "图片";
    case "SCREENSHOT": return "截图";
    case "FILES": return "文件";
    case "GIF": return "GIF";
    case "VIDEO": return "视频";
  }
}

function formatRelativeTime(ms: number): string {
  const diff = Date.now() - ms;
  const seconds = Math.floor(diff / 1000);
  if (seconds < 45) return "刚刚";
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}分钟前`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}小时前`;
  const days = Math.floor(hours / 24);
  if (days === 1) return "昨天";
  if (days < 7) return `${days}天前`;
  return new Date(ms).toLocaleDateString();
}

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}
