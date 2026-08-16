import { useEffect, useRef, useState, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useToast } from "../components/Toast";
import { CheckIcon, XIcon, CropIcon } from "../components/icons";

type Tool = "rectangle" | "arrow" | "mosaic" | "blur";

interface Ann {
  id: string;
  kind: Tool;
  geometry: number[];
  style: Record<string, unknown>;
  z_index: number;
}

interface Source {
  data_url: string;
  width: number;
  height: number;
}

export function AnnotatePage({ itemId, onDone }: { itemId: string; onDone: () => void }) {
  const { success, error: showError } = useToast();
  const [source, setSource] = useState<Source | null>(null);
  const [items, setItems] = useState<Ann[]>([]);
  const [undo, setUndo] = useState<Ann[][]>([]);
  const [redo, setRedo] = useState<Ann[][]>([]);
  const [tool, setTool] = useState<Tool>("rectangle");
  const [isSaving, setIsSaving] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const surface = useRef<HTMLDivElement>(null);
  const start = useRef<[number, number] | null>(null);

  const loadSource = useCallback(async () => {
    setSource(null);
    setLoadError(null);
    setItems([]);
    setUndo([]);
    setRedo([]);
    try {
      setSource(await invoke<Source>("annotation_source", { itemId }));
    } catch (e) {
      const message = String(e);
      setLoadError(message);
      showError(`读取待标注画面失败: ${message}`);
    }
  }, [itemId, showError]);

  useEffect(() => {
    void loadSource();
  }, [loadSource]);

  const push = useCallback((next: Ann[]) => {
    setUndo((history) => [...history, items]);
    setRedo([]);
    setItems(next);
  }, [items]);

  const undoOnce = useCallback(() => {
    const previous = undo.at(-1);
    if (!previous) return;
    setUndo((history) => history.slice(0, -1));
    setRedo((history) => [...history, items]);
    setItems(previous);
  }, [undo, items]);

  const redoOnce = useCallback(() => {
    const next = redo.at(-1);
    if (!next) return;
    setRedo((history) => history.slice(0, -1));
    setUndo((history) => [...history, items]);
    setItems(next);
  }, [redo, items]);

  // Keyboard shortcuts (⌘Z, ⌘⇧Z, Esc, Enter)
  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      if ((e.metaKey || e.ctrlKey) && e.key === "z") {
        e.preventDefault();
        if (e.shiftKey) {
          redoOnce();
        } else {
          undoOnce();
        }
      } else if (e.key === "Escape") {
        e.preventDefault();
        onDone();
      }
    }
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [undoOnce, redoOnce, onDone]);

  function imagePoint(clientX: number, clientY: number): [number, number] | null {
    if (!source || !surface.current) return null;
    const rect = surface.current.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) return null;
    const x = ((clientX - rect.left) * source.width) / rect.width;
    const y = ((clientY - rect.top) * source.height) / rect.height;
    return [Math.max(0, Math.min(source.width, x)), Math.max(0, Math.min(source.height, y))];
  }

  async function handleExport() {
    if (!source) return;
    try {
      setIsSaving(true);
      await invoke("export_annotated", { itemId, scene: { items } });
      success("标注已完成并保存至历史");
      onDone();
    } catch (e) {
      showError(`保存失败: ${e}`);
    } finally {
      setIsSaving(false);
    }
  }

  return (
    <div
      style={{
        position: "fixed",
        inset: 0,
        backgroundColor: "rgba(8, 12, 20, 0.95)",
        backdropFilter: "blur(16px)",
        display: "flex",
        flexDirection: "column",
        zIndex: 1100,
      }}
    >
      {/* Top Floating Glass Toolbar */}
      <header
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          padding: "14px 28px",
          borderBottom: "1px solid var(--border-subtle)",
          background: "rgba(13, 18, 31, 0.8)",
          backdropFilter: "blur(12px)",
        }}
      >
        <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
          <div className="brand-icon" style={{ width: 26, height: 26 }}>
            <CropIcon size={15} />
          </div>
          <span style={{ fontSize: 15, fontWeight: 600 }}>图片标注画布</span>
        </div>

        {/* Tools */}
        <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
          {(["rectangle", "arrow", "mosaic", "blur"] as Tool[]).map((t) => (
            <button
              key={t}
              className={`filter-chip ${tool === t ? "active" : ""}`}
              onClick={() => setTool(t)}
            >
              <span>{toolLabel(t)}</span>
            </button>
          ))}
          <div style={{ width: 1, height: 20, background: "var(--border-subtle)", margin: "0 4px" }} />
          <button className="btn btn-secondary" disabled={undo.length === 0} onClick={undoOnce} title="撤销 (⌘Z)">
            <span>撤销</span>
          </button>
          <button className="btn btn-secondary" disabled={redo.length === 0} onClick={redoOnce} title="重做 (⌘⇧Z)">
            <span>重做</span>
          </button>
        </div>

        {/* Actions */}
        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          <button className="btn btn-ghost" onClick={onDone}>
            <XIcon size={15} />
            <span>取消</span>
          </button>
          <button className="btn btn-primary" disabled={!source || isSaving} onClick={handleExport}>
            <CheckIcon size={15} />
            <span>{isSaving ? "正在生成…" : "完成并存入剪贴板"}</span>
          </button>
        </div>
      </header>

      {/* Main Canvas Area */}
      <div
        style={{
          flex: 1,
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          overflow: "auto",
          padding: 24,
          background: "radial-gradient(circle at center, #111a2e 0%, #080c14 100%)",
        }}
      >
        {!source && !loadError && (
          <div className="empty-state">
            <div className="empty-state-title">正在读取待标注画面…</div>
          </div>
        )}

        {!source && loadError && (
          <div className="empty-state">
            <div className="empty-state-title" style={{ color: "var(--danger)" }}>
              待读取画面加载失败
            </div>
            <div className="empty-state-sub">{loadError}</div>
            <button className="btn btn-secondary" onClick={() => void loadSource()}>
              重试
            </button>
          </div>
        )}

        {source && (
          <div
            ref={surface}
            style={{
              position: "relative",
              display: "inline-block",
              maxWidth: "100%",
              maxHeight: "100%",
              lineHeight: 0,
              cursor: "crosshair",
              boxShadow: "0 12px 48px rgba(0, 0, 0, 0.7)",
              borderRadius: 6,
              overflow: "hidden",
              border: "1px solid var(--border-hover)",
            }}
            onPointerDown={(event) => {
              start.current = imagePoint(event.clientX, event.clientY);
              event.currentTarget.setPointerCapture(event.pointerId);
            }}
            onPointerUp={(event) => {
              const from = start.current;
              const to = imagePoint(event.clientX, event.clientY);
              start.current = null;
              if (!from || !to) return;
              const [x0, y0] = from;
              const [x1, y1] = to;
              const geometry =
                tool === "arrow"
                  ? [x0, y0, x1, y1]
                  : [
                      Math.min(x0, x1),
                      Math.min(y0, y1),
                      Math.max(2, Math.abs(x1 - x0)),
                      Math.max(2, Math.abs(y1 - y0)),
                    ];
              push([
                ...items,
                {
                  id: crypto.randomUUID(),
                  kind: tool,
                  geometry,
                  style: {},
                  z_index: items.length,
                },
              ]);
            }}
          >
            <img
              src={source.data_url}
              alt="待标注截图"
              draggable={false}
              style={{
                display: "block",
                maxWidth: "100%",
                maxHeight: "calc(100vh - 120px)",
                objectFit: "contain",
                userSelect: "none",
              }}
            />
            <svg
              viewBox={`0 0 ${source.width} ${source.height}`}
              style={{
                position: "absolute",
                inset: 0,
                width: "100%",
                height: "100%",
                pointerEvents: "none",
              }}
            >
              <defs>
                <marker id="arrowhead" markerWidth="10" markerHeight="7" refX="9" refY="3.5" orient="auto">
                  <polygon points="0 0, 10 3.5, 0 7" fill="#ef4444" />
                </marker>
              </defs>
              {items.map((ann) => (
                <AnnotationPreview key={ann.id} annotation={ann} />
              ))}
            </svg>
          </div>
        )}
      </div>
    </div>
  );
}

function toolLabel(tool: Tool): string {
  switch (tool) {
    case "rectangle": return "矩形选框";
    case "arrow": return "指向箭头";
    case "mosaic": return "马赛克";
    case "blur": return "高斯模糊";
  }
}

function AnnotationPreview({ annotation }: { annotation: Ann }) {
  const [x, y, a, b] = annotation.geometry;
  if (annotation.kind === "arrow") {
    return <line x1={x} y1={y} x2={a} y2={b} stroke="#ef4444" strokeWidth="3.5" markerEnd="url(#arrowhead)" />;
  }
  const fill = annotation.kind === "rectangle" ? "none" : "rgba(148, 163, 184, 0.4)";
  return (
    <rect
      x={x}
      y={y}
      width={a}
      height={b}
      fill={fill}
      stroke="#ef4444"
      strokeWidth="3.5"
      rx="3"
    />
  );
}
