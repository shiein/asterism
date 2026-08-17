import { useEffect, useRef, useState, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useToast } from "../components/Toast";
import { CheckIcon, XIcon, CropIcon } from "../components/icons";

type Tool = "rectangle" | "ellipse" | "arrow" | "brush" | "mosaic" | "text";

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
  const [currentStroke, setCurrentStroke] = useState<number[]>([]);
  
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
      success("标注已完成并保存至剪贴板与历史");
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
        backgroundColor: "rgba(15, 23, 42, 0.85)",
        backdropFilter: "blur(16px)",
        display: "flex",
        flexDirection: "column",
        zIndex: 1100,
      }}
    >
      {/* Header Toolbar */}
      <header
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          padding: "12px 24px",
          borderBottom: "1px solid var(--border-subtle)",
          background: "var(--bg-card)",
          boxShadow: "var(--shadow-sm)",
        }}
      >
        <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
          <div className="brand-icon" style={{ width: 26, height: 26 }}>
            <CropIcon size={15} />
          </div>
          <span style={{ fontSize: 15, fontWeight: 600, color: "var(--text-primary)" }}>标注工作室</span>
        </div>

        {/* Tools */}
        <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
          {(["rectangle", "ellipse", "arrow", "brush", "mosaic", "text"] as Tool[]).map((t) => (
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
            <span>{isSaving ? "正在生成…" : "完成并复制"}</span>
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
          background: "var(--bg-app)",
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
              boxShadow: "var(--shadow-lg)",
              borderRadius: 6,
              overflow: "hidden",
              border: "1px solid var(--border-hover)",
              touchAction: "none",
            }}
            onPointerDown={(event) => {
              const pt = imagePoint(event.clientX, event.clientY);
              if (!pt) return;
              start.current = pt;
              setCurrentStroke([pt[0], pt[1]]);
              event.currentTarget.setPointerCapture(event.pointerId);
            }}
            onPointerMove={(event) => {
              if (!start.current) return;
              const pt = imagePoint(event.clientX, event.clientY);
              if (!pt) return;
              if (tool === "brush" || tool === "mosaic") {
                setCurrentStroke((prev) => [...prev, pt[0], pt[1]]);
              }
            }}
            onPointerUp={(event) => {
              const from = start.current;
              const to = imagePoint(event.clientX, event.clientY);
              start.current = null;
              if (!from || !to) return;
              const [x0, y0] = from;
              const [x1, y1] = to;

              let geometry: number[];
              if (tool === "brush" || tool === "mosaic") {
                geometry = currentStroke.length >= 2 ? currentStroke : [x0, y0, x1, y1];
              } else if (tool === "arrow") {
                geometry = [x0, y0, x1, y1];
              } else if (tool === "text") {
                geometry = [x0, y0];
              } else {
                geometry = [
                  Math.min(x0, x1),
                  Math.min(y0, y1),
                  Math.max(2, Math.abs(x1 - x0)),
                  Math.max(2, Math.abs(y1 - y0)),
                ];
              }

              push([
                ...items,
                {
                  id: crypto.randomUUID(),
                  kind: tool,
                  geometry,
                  style: {
                    stroke_width: 3.5,
                    brush_radius: 14.0,
                    block_size: 12,
                    text: tool === "text" ? "标注文本" : undefined,
                  },
                  z_index: items.length,
                },
              ]);
              setCurrentStroke([]);
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
                <pattern id="mosaic-pat" width="12" height="12" patternUnits="userSpaceOnUse">
                  <rect width="6" height="6" fill="rgba(148, 163, 184, 0.45)" />
                  <rect x="6" width="6" height="6" fill="rgba(100, 116, 139, 0.45)" />
                  <rect y="6" width="6" height="6" fill="rgba(100, 116, 139, 0.45)" />
                  <rect x="6" y="6" width="6" height="6" fill="rgba(148, 163, 184, 0.45)" />
                </pattern>
              </defs>
              {items.map((ann) => (
                <AnnotationPreview key={ann.id} annotation={ann} />
              ))}
              {currentStroke.length >= 2 && (tool === "brush" || tool === "mosaic") && (
                <polyline
                  points={currentStroke.reduce((acc, val, i) => `${acc}${i % 2 === 0 ? " " : ","}${val}`, "").trim()}
                  fill="none"
                  stroke={tool === "mosaic" ? "url(#mosaic-pat)" : "#ef4444"}
                  strokeWidth={tool === "mosaic" ? 24 : 4}
                  strokeLinecap="round"
                  strokeLinejoin="round"
                />
              )}
            </svg>
          </div>
        )}
      </div>
    </div>
  );
}

function toolLabel(tool: Tool): string {
  switch (tool) {
    case "rectangle": return "矩形";
    case "ellipse": return "椭圆";
    case "arrow": return "箭头";
    case "brush": return "画笔";
    case "mosaic": return "马赛克 (实时画笔)";
    case "text": return "文字";
  }
}

function AnnotationPreview({ annotation }: { annotation: Ann }) {
  if (annotation.kind === "arrow") {
    const [x1, y1, x2, y2] = annotation.geometry;
    return <line x1={x1} y1={y1} x2={x2} y2={y2} stroke="#ef4444" strokeWidth="3.5" markerEnd="url(#arrowhead)" />;
  }

  if (annotation.kind === "ellipse") {
    const [x, y, w, h] = annotation.geometry;
    return (
      <ellipse
        cx={x + w / 2}
        cy={y + h / 2}
        rx={w / 2}
        ry={h / 2}
        fill="none"
        stroke="#ef4444"
        strokeWidth="3.5"
      />
    );
  }

  if (annotation.kind === "brush") {
    const points = annotation.geometry.reduce((acc, val, i) => `${acc}${i % 2 === 0 ? " " : ","}${val}`, "").trim();
    return <polyline points={points} fill="none" stroke="#ef4444" strokeWidth="4" strokeLinecap="round" strokeLinejoin="round" />;
  }

  if (annotation.kind === "mosaic") {
    if (annotation.geometry.length === 4) {
      const [x, y, w, h] = annotation.geometry;
      return <rect x={x} y={y} width={w} height={h} fill="url(#mosaic-pat)" stroke="rgba(100, 116, 139, 0.4)" strokeWidth="1" />;
    }

    const points = annotation.geometry.reduce((acc, val, i) => `${acc}${i % 2 === 0 ? " " : ","}${val}`, "").trim();
    return <polyline points={points} fill="none" stroke="url(#mosaic-pat)" strokeWidth="24" strokeLinecap="round" strokeLinejoin="round" />;
  }

  if (annotation.kind === "text") {
    const [x, y] = annotation.geometry;
    return (
      <text x={x} y={y + 16} fill="#ef4444" fontSize="16" fontWeight="bold" fontFamily="sans-serif">
        {String(annotation.style.text || "标注文本")}
      </text>
    );
  }

  const [x, y, a, b] = annotation.geometry;
  return (
    <rect
      x={x}
      y={y}
      width={a}
      height={b}
      fill="none"
      stroke="#ef4444"
      strokeWidth="3.5"
      rx="3"
    />
  );
}

