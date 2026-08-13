import { useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

type Tool = "rectangle" | "arrow" | "mosaic" | "blur";

interface Ann {
  id: string;
  kind: Tool;
  geometry: number[];
  style: Record<string, unknown>;
  z_index: number;
}

export function AnnotatePage({ itemId, onDone }: { itemId: string; onDone: () => void }) {
  const [items, setItems] = useState<Ann[]>([]);
  const [undo, setUndo] = useState<Ann[][]>([]);
  const [tool, setTool] = useState<Tool>("rectangle");
  const start = useRef<[number, number] | null>(null);

  function push(next: Ann[]) {
    setUndo((u) => [...u, items]);
    setItems(next);
  }

  return (
    <section className="card">
      <h2>标注</h2>
      <div className="actions">
        {(["rectangle", "arrow", "mosaic", "blur"] as Tool[]).map((t) => (
          <button key={t} className={tool === t ? "chip on" : "chip"} onClick={() => setTool(t)}>
            {t}
          </button>
        ))}
        <button
          onClick={() => {
            const prev = undo[undo.length - 1];
            if (!prev) return;
            setUndo((u) => u.slice(0, -1));
            setItems(prev);
          }}
        >
          Undo
        </button>
        <button
          onClick={() =>
            void invoke("export_annotated", { itemId, scene: { items } }).then(() => onDone())
          }
        >
          导出
        </button>
      </div>
      <div
        className="card"
        style={{ minHeight: 240, position: "relative" }}
        onMouseDown={(e) => {
          const r = (e.currentTarget as HTMLDivElement).getBoundingClientRect();
          start.current = [e.clientX - r.left, e.clientY - r.top];
        }}
        onMouseUp={(e) => {
          if (!start.current) return;
          const r = (e.currentTarget as HTMLDivElement).getBoundingClientRect();
          const [x0, y0] = start.current;
          const x1 = e.clientX - r.left;
          const y1 = e.clientY - r.top;
          start.current = null;
          push([
            ...items,
            {
              id: crypto.randomUUID(),
              kind: tool,
              geometry: [x0, y0, Math.abs(x1 - x0), Math.abs(y1 - y0)],
              style: {},
              z_index: items.length,
            },
          ]);
        }}
      >
        <p className="muted">在区域内拖拽添加 {tool}，导出由 Rust 合成。</p>
        <ul>
          {items.map((a) => (
            <li key={a.id}>
              {a.kind} {a.geometry.map((n) => n.toFixed(0)).join(",")}
            </li>
          ))}
        </ul>
      </div>
    </section>
  );
}
