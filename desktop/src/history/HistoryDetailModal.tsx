import { useState } from "react";
import { Modal } from "../components/Modal";
import { CopyIcon, StarIcon, TrashIcon, EditIcon, CheckIcon } from "../components/icons";
import { useToast } from "../components/Toast";
import type { HistoryItem } from "../types";

interface HistoryDetailModalProps {
  item: HistoryItem | null;
  isOpen: boolean;
  onClose: () => void;
  onCopy: (id: string) => void;
  onFavorite: (id: string, favorite: boolean) => void;
  onDelete: (id: string) => void;
  onAnnotate: (id: string) => void;
  imageUrl?: string;
}

export function HistoryDetailModal({
  item,
  isOpen,
  onClose,
  onCopy,
  onFavorite,
  onDelete,
  onAnnotate,
  imageUrl,
}: HistoryDetailModalProps) {
  const { success } = useToast();
  const [copied, setCopied] = useState(false);

  if (!item) return null;

  function handleCopy() {
    if (!item) return;
    onCopy(item.id);
    setCopied(true);
    success("已复制到剪贴板");
    setTimeout(() => setCopied(false), 1500);
  }

  const isVisual = item.kind === "IMAGE" || item.kind === "SCREENSHOT" || item.kind === "GIF";

  return (
    <Modal
      isOpen={isOpen}
      onClose={onClose}
      title={`历史详情 · ${formatKind(item.kind)}`}
      maxWidth={740}
      footer={
        <div style={{ display: "flex", justifyContent: "space-between", width: "100%" }}>
          <button
            className="btn btn-danger"
            onClick={() => {
              onDelete(item.id);
              onClose();
            }}
          >
            <TrashIcon size={15} />
            <span>删除记录</span>
          </button>
          <div style={{ display: "flex", gap: 8 }}>
            {isVisual && (
              <button
                className="btn btn-secondary"
                onClick={() => {
                  onClose();
                  onAnnotate(item.id);
                }}
              >
                <EditIcon size={15} />
                <span>进入标注</span>
              </button>
            )}
            <button
              className="btn btn-secondary"
              onClick={() => onFavorite(item.id, !item.favorite)}
            >
              <StarIcon size={15} filled={item.favorite} />
              <span>{item.favorite ? "取消收藏" : "加入收藏"}</span>
            </button>
            <button className="btn btn-primary" onClick={handleCopy}>
              {copied ? <CheckIcon size={15} /> : <CopyIcon size={15} />}
              <span>{copied ? "已复制" : "复制正文"}</span>
            </button>
          </div>
        </div>
      }
    >
      <div style={{ display: "flex", flexDirection: "column", gap: 16 }}>
        {/* Meta Info Bar */}
        <div
          style={{
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
            padding: "10px 14px",
            background: "rgba(255, 255, 255, 0.03)",
            borderRadius: "var(--radius-md)",
            fontSize: 12,
            color: "var(--text-secondary)",
          }}
        >
          <div>
            创建时间：<span style={{ color: "var(--text-primary)" }}>{new Date(item.createdAtMs).toLocaleString()}</span>
          </div>
          {item.sourceApp && (
            <div>
              来源应用：<span style={{ color: "var(--text-primary)" }}>{item.sourceApp}</span>
            </div>
          )}
          <div>
            大小：<span style={{ color: "var(--text-primary)" }}>{formatBytes(item.logicalSize)}</span>
          </div>
        </div>

        {/* Content Viewer */}
        {item.kind === "TEXT" && (
          <div
            style={{
              fontFamily: "var(--font-mono)",
              fontSize: 13,
              lineHeight: 1.6,
              padding: "14px 16px",
              background: "rgba(0, 0, 0, 0.35)",
              border: "1px solid var(--border-subtle)",
              borderRadius: "var(--radius-md)",
              whiteSpace: "pre-wrap",
              wordBreak: "break-all",
              maxHeight: 380,
              overflowY: "auto",
              color: "#f1f5f9",
              userSelect: "text",
            }}
          >
            {item.preview || "无预览文本"}
          </div>
        )}

        {isVisual && (
          <div
            style={{
              display: "flex",
              flexDirection: "column",
              alignItems: "center",
              justifyContent: "center",
              padding: 16,
              background: "rgba(0, 0, 0, 0.4)",
              borderRadius: "var(--radius-md)",
              border: "1px solid var(--border-subtle)",
              gap: 12,
            }}
          >
            {imageUrl ? (
              <img
                src={imageUrl}
                alt="图片预览"
                style={{
                  maxWidth: "100%",
                  maxHeight: 360,
                  objectFit: "contain",
                  borderRadius: 6,
                  boxShadow: "0 4px 16px rgba(0,0,0,0.5)",
                }}
              />
            ) : (
              <div style={{ color: "var(--text-muted)", fontSize: 13 }}>加载大图预览中…</div>
            )}
            {item.imageWidth && item.imageHeight && (
              <div style={{ fontSize: 12, color: "var(--text-muted)" }}>
                分辨率：{item.imageWidth} × {item.imageHeight} px
              </div>
            )}
          </div>
        )}

        {item.kind === "FILES" && (
          <div
            style={{
              padding: "16px 20px",
              background: "rgba(0, 0, 0, 0.3)",
              borderRadius: "var(--radius-md)",
              border: "1px solid var(--border-subtle)",
              display: "flex",
              flexDirection: "column",
              gap: 8,
            }}
          >
            <div style={{ fontSize: 14, fontWeight: 600, color: "var(--text-primary)" }}>
              {item.preview || "文件列表"}
            </div>
            <div style={{ fontSize: 12, color: "var(--text-muted)" }}>
              包含 {item.fileCount ?? 1} 个文件 · 总大小 {formatBytes(item.logicalSize)}
            </div>
          </div>
        )}
      </div>
    </Modal>
  );
}

function formatKind(kind: string): string {
  switch (kind) {
    case "TEXT": return "纯文本";
    case "IMAGE": return "图片文件";
    case "SCREENSHOT": return "屏幕截屏";
    case "FILES": return "文件传输";
    case "GIF": return "GIF 动图";
    case "VIDEO": return "高清录像";
    default: return kind;
  }
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}
