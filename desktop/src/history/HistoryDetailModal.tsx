import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { pinImage, previewImage } from "../api";
import { Modal } from "../components/Modal";
import { CopyIcon, StarIcon, TrashIcon, EditIcon, CheckIcon, PinIcon } from "../components/icons";
import type { HistoryItem } from "../types";

interface HistoryDetailModalProps {
  item: HistoryItem | null;
  isOpen: boolean;
  onClose: () => void;
  canCopy: boolean;
  canFavorite: boolean;
  canDelete: boolean;
  onCopy: (id: string) => Promise<void>;
  onFavorite: (id: string, favorite: boolean) => Promise<void>;
  onDelete: (id: string) => Promise<void>;
  onAnnotate: (id: string) => void;
}

export function HistoryDetailModal({
  item,
  isOpen,
  onClose,
  canCopy,
  canFavorite,
  canDelete,
  onCopy,
  onFavorite,
  onDelete,
  onAnnotate,
}: HistoryDetailModalProps) {
  const [copied, setCopied] = useState(false);
  const [busy, setBusy] = useState<"copy" | "favorite" | "delete" | null>(null);

  const isVisual =
    item?.kind === "IMAGE" || item?.kind === "SCREENSHOT" || item?.kind === "GIF";
  const image = useQuery({
    queryKey: ["preview-image", item?.id],
    queryFn: () => previewImage(item!.id),
    enabled: isOpen && Boolean(item) && isVisual,
    staleTime: 120_000,
  });

  if (!item) return null;

  async function run(kind: "copy" | "favorite" | "delete") {
    if (!item) return;
    try {
      setBusy(kind);
      if (kind === "copy") {
        await onCopy(item.id);
        setCopied(true);
        setTimeout(() => setCopied(false), 1500);
      } else if (kind === "favorite") {
        await onFavorite(item.id, !item.favorite);
      } else {
        await onDelete(item.id);
        onClose();
      }
    } catch {
      // 错误提示由发起 mutation 的一方负责。
    } finally {
      setBusy(null);
    }
  }

  return (
    <Modal
      isOpen={isOpen}
      onClose={onClose}
      title={`详情 · ${formatKind(item.kind)}`}
      maxWidth={680}
      footer={
        <>
          {canDelete && (
            <button
              className="btn btn-danger"
              disabled={busy !== null}
              onClick={() => void run("delete")}
            >
              <TrashIcon size={14} />
              <span>{busy === "delete" ? "删除中…" : "删除"}</span>
            </button>
          )}
          <div className="spacer" />
          {isVisual && (
            <>
              <button
                className="btn"
                onClick={() => {
                  onClose();
                  void pinImage(item.id);
                }}
                title="贴在屏幕上 (Pin)"
              >
                <PinIcon size={14} />
                <span>贴图</span>
              </button>
              <button
                className="btn"
                onClick={() => {
                  onClose();
                  onAnnotate(item.id);
                }}
              >
                <EditIcon size={14} />
                <span>标注</span>
              </button>
            </>
          )}
          {canFavorite && (
            <button
              className="btn"
              disabled={busy !== null}
              onClick={() => void run("favorite")}
            >
              <StarIcon size={14} filled={item.favorite} />
              <span>{item.favorite ? "取消收藏" : "收藏"}</span>
            </button>
          )}
          {canCopy && (
            <button
              className="btn btn-primary"
              disabled={busy !== null}
              onClick={() => void run("copy")}
            >
              {copied ? <CheckIcon size={14} /> : <CopyIcon size={14} />}
              <span>{busy === "copy" ? "复制中…" : copied ? "已复制" : "复制"}</span>
            </button>
          )}
        </>
      }
    >
      <div style={{ display: "grid", gap: 14 }}>
        <dl className="meta-grid">
          <div>
            <dt>创建时间</dt>
            <dd>{new Date(item.createdAtMs).toLocaleString()}</dd>
          </div>
          {item.sourceApp && (
            <div>
              <dt>来源应用</dt>
              <dd>{item.sourceApp}</dd>
            </div>
          )}
          <div>
            <dt>大小</dt>
            <dd>{formatBytes(item.logicalSize)}</dd>
          </div>
          {item.imageWidth && item.imageHeight && (
            <div>
              <dt>分辨率</dt>
              <dd>
                {item.imageWidth} × {item.imageHeight}
              </dd>
            </div>
          )}
          {item.kind === "FILES" && (
            <div>
              <dt>文件数</dt>
              <dd>{item.fileCount ?? 1}</dd>
            </div>
          )}
        </dl>

        {item.kind === "TEXT" && (
          <div
            className="text-block"
            style={{
              maxHeight: 360,
              overflowY: "auto",
              display: "block",
              WebkitLineClamp: "unset",
              userSelect: "text",
            }}
          >
            {item.preview || "（无预览）"}
          </div>
        )}

        {isVisual && (
          <div className="viewer">
            {image.data ? (
              <img src={image.data} alt="预览" />
            ) : image.isError ? (
              <div className="row">
                <span style={{ color: "var(--danger)" }}>预览加载失败</span>
                <button className="btn" onClick={() => void image.refetch()}>
                  重试
                </button>
              </div>
            ) : (
              <span style={{ color: "var(--text-tertiary)" }}>载入预览…</span>
            )}
          </div>
        )}

        {item.kind === "FILES" && (
          <div className="file-block">
            <div>
              <div className="list-row-title">{item.preview || "文件列表"}</div>
              <div className="list-row-sub">
                {item.fileCount ?? 1} 个文件 · {formatBytes(item.logicalSize)}
              </div>
            </div>
          </div>
        )}
      </div>
    </Modal>
  );
}

function formatKind(kind: string): string {
  switch (kind) {
    case "TEXT":
      return "文本";
    case "IMAGE":
      return "图片";
    case "SCREENSHOT":
      return "截图";
    case "FILES":
      return "文件";
    case "GIF":
      return "GIF";
    case "VIDEO":
      return "视频";
    default:
      return kind;
  }
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}
