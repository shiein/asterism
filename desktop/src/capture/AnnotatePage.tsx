import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

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
  const [source, setSource] = useState<Source | null>(null);
  const [items, setItems] = useState<Ann[]>([]);
  const [undo, setUndo] = useState<Ann[][]>([]);
  const [redo, setRedo] = useState<Ann[][]>([]);
  const [tool, setTool] = useState<Tool>("rectangle");
  const [error, setError] = useState<string | null>(null);
  const surface = useRef<HTMLDivElement>(null);
  const start = useRef<[number, number] | null>(null);

  useEffect(() => {
    setSource(null);
    setItems([]);
    setUndo([]);
    setRedo([]);
    setError(null);
    void invoke<Source>("annotation_source", { itemId }).then(setSource).catch((e) => setError(String(e)));
  }, [itemId]);

  function push(next: Ann[]) {
    setUndo((history) => [...history, items]);
    setRedo([]);
    setItems(next);
  }

  function imagePoint(clientX: number, clientY: number): [number, number] | null {
    if (!source || !surface.current) return null;
    const rect = surface.current.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) return null;
    const x = ((clientX - rect.left) * source.width) / rect.width;
    const y = ((clientY - rect.top) * source.height) / rect.height;
    return [Math.max(0, Math.min(source.width, x)), Math.max(0, Math.min(source.height, y))];
  }

  function undoOnce() {
    const previous = undo.at(-1);
    if (!previous) return;
    setUndo((history) => history.slice(0, -1));
    setRedo((history) => [...history, items]);
    setItems(previous);
  }

  function redoOnce() {
    const next = redo.at(-1);
    if (!next) return;
    setRedo((history) => history.slice(0, -1));
    setUndo((history) => [...history, items]);
    setItems(next);
  }

  return (
    <section className="card">
      <h2>标注</h2>
      <div className="actions">
        {(["rectangle", "arrow", "mosaic", "blur"] as Tool[]).map((candidate) => (
          <button
            key={candidate}
            className={tool === candidate ? "chip on" : "chip"}
            onClick={() => setTool(candidate)}
          >
            {toolLabel(candidate)}
          </button>
        ))}
        <button disabled={undo.length === 0} onClick={undoOnce}>Undo</button>
        <button disabled={redo.length === 0} onClick={redoOnce}>Redo</button>
        <button
          disabled={!source}
          onClick={() => {
            setError(null);
            void invoke("export_annotated", { itemId, scene: { items } })
              .then(() => onDone())
              .catch((e) => setError(String(e)));
          }}
        >
          完成
        </button>
        <button onClick={onDone}>取消</button>
      </div>
      {error && <p className="error">{error}</p>}
      {!source && !error && <p className="muted">读取截图…</p>}
      {source && (
        <div
          ref={surface}
          className="card"
          style={{ position: "relative", display: "inline-block", maxWidth: "100%", padding: 0, lineHeight: 0 }}
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
            const geometry = tool === "arrow"
              ? [x0, y0, x1, y1]
              : [Math.min(x0, x1), Math.min(y0, y1), Math.max(1, Math.abs(x1 - x0)), Math.max(1, Math.abs(y1 - y0))];
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
            style={{ display: "block", width: "auto", height: "auto", maxWidth: "100%", userSelect: "none" }}
          />
          <svg
            viewBox={`0 0 ${source.width} ${source.height}`}
            style={{ position: "absolute", inset: 0, width: "100%", height: "100%", pointerEvents: "none" }}
          >
            <defs>
              <marker id="arrowhead" markerWidth="10" markerHeight="7" refX="9" refY="3.5" orient="auto">
                <polygon points="0 0, 10 3.5, 0 7" fill="#ff4646" />
              </marker>
            </defs>
            {items.map((annotation) => <AnnotationPreview key={annotation.id} annotation={annotation} />)}
          </svg>
        </div>
      )}
    </section>
  );
}

function toolLabel(tool: Tool): string {
  switch (tool) {
    case "rectangle":
      return "矩形";
    case "arrow":
      return "箭头";
    case "mosaic":
      return "马赛克";
    case "blur":
      return "模糊";
  }
}

function AnnotationPreview({ annotation }: { annotation: Ann }) {
  const [x, y, a, b] = annotation.geometry;
  if (annotation.kind === "arrow") {
    return <line x1={x} y1={y} x2={a} y2={b} stroke="#ff4646" strokeWidth="3" markerEnd="url(#arrowhead)" />;
  }
  const fill = annotation.kind === "rectangle" ? "none" : "rgba(160, 160, 160, 0.45)";
  return <rect x={x} y={y} width={a} height={b} fill={fill} stroke="#ff4646" strokeWidth="3" />;
}
