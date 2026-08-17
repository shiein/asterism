import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useToast } from "../components/Toast";
import {
  CheckIcon,
  XIcon,
  EditIcon,
  SquareIcon,
  CircleIcon,
  ArrowIcon,
  PenIcon,
  MosaicIcon,
  TypeIcon,
  UndoIcon,
  RedoIcon,
} from "../components/icons";

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

/** 标注墨色。必须与 Rust `annotation::draw_annotation` 的默认色 (255,59,48) 一致。 */
const MARK_COLOR = "#ff3b30";

const TOOLS: Array<{ key: Tool; label: string; icon: React.ReactNode }> = [
  { key: "rectangle", label: "矩形", icon: <SquareIcon size={15} /> },
  { key: "ellipse", label: "椭圆", icon: <CircleIcon size={15} /> },
  { key: "arrow", label: "箭头", icon: <ArrowIcon size={15} /> },
  { key: "brush", label: "画笔", icon: <PenIcon size={15} /> },
  { key: "mosaic", label: "马赛克", icon: <MosaicIcon size={15} /> },
  { key: "text", label: "文字（仅 ASCII）", icon: <TypeIcon size={15} /> },
];

/// 与 Rust 侧 `annotation::mosaic_mask` 保持同一套取值规则，
/// 保证预览看到的糊法就是导出结果。
function mosaicMetrics(source: Source) {
  const block = Math.max(8, Math.round(Math.min(source.width, source.height) / 90));
  return { block, radius: block * 1.5 };
}

function strokeWidth(source: Source) {
  return Math.max(2, Math.round(Math.min(source.width, source.height) / 320));
}

function fontScale(source: Source) {
  return Math.max(2, Math.round(Math.min(source.width, source.height) / 220));
}

export function AnnotatePage({ itemId, onDone }: { itemId: string; onDone: () => void }) {
  const { success, error: showError } = useToast();
  const [source, setSource] = useState<Source | null>(null);
  const [items, setItems] = useState<Ann[]>([]);
  const [undoStack, setUndoStack] = useState<Ann[][]>([]);
  const [redoStack, setRedoStack] = useState<Ann[][]>([]);
  const [tool, setTool] = useState<Tool>("rectangle");
  const [isSaving, setIsSaving] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [draftStroke, setDraftStroke] = useState<number[] | null>(null);
  const [textDraft, setTextDraft] = useState<{ x: number; y: number; value: string } | null>(null);
  const [asciiWarning, setAsciiWarning] = useState(false);

  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const imageRef = useRef<HTMLImageElement | null>(null);
  const dragStart = useRef<[number, number] | null>(null);

  const loadSource = useCallback(async () => {
    setSource(null);
    setLoadError(null);
    setItems([]);
    setUndoStack([]);
    setRedoStack([]);
    try {
      setSource(await invoke<Source>("annotation_source", { itemId }));
    } catch (e) {
      const message = String(e);
      setLoadError(message);
      showError(`读取待标注画面失败：${message}`);
    }
  }, [itemId, showError]);

  useEffect(() => {
    void loadSource();
  }, [loadSource]);

  // 解码原图，之后每次重绘都从它开始，避免在已经画过的画面上叠加。
  useEffect(() => {
    if (!source) {
      imageRef.current = null;
      return;
    }
    const image = new Image();
    image.onload = () => {
      imageRef.current = image;
      setItems((current) => [...current]);
    };
    image.src = source.data_url;
  }, [source]);

  const commit = useCallback(
    (next: Ann[]) => {
      setUndoStack((history) => [...history, items]);
      setRedoStack([]);
      setItems(next);
    },
    [items]
  );

  const undo = useCallback(() => {
    const previous = undoStack.at(-1);
    if (!previous) return;
    setUndoStack((history) => history.slice(0, -1));
    setRedoStack((history) => [...history, items]);
    setItems(previous);
  }, [undoStack, items]);

  const redo = useCallback(() => {
    const next = redoStack.at(-1);
    if (!next) return;
    setRedoStack((history) => history.slice(0, -1));
    setUndoStack((history) => [...history, items]);
    setItems(next);
  }, [redoStack, items]);

  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      if (textDraft) return;
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "z") {
        e.preventDefault();
        if (e.shiftKey) redo();
        else undo();
      } else if (e.key === "Escape") {
        e.preventDefault();
        onDone();
      }
    }
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [undo, redo, onDone, textDraft]);

  const liveAnnotation = useMemo<Ann | null>(() => {
    if (!source || !draftStroke || draftStroke.length < 2) return null;
    return buildAnnotation(tool, draftStroke, source, "__live__", items.length);
  }, [draftStroke, tool, source, items.length]);

  const draftText = useMemo<Ann | null>(() => {
    if (!source || !textDraft || !textDraft.value) return null;
    return {
      id: "__draft__",
      kind: "text",
      geometry: [textDraft.x, textDraft.y],
      style: { text: textDraft.value, font_scale: fontScale(source) },
      z_index: Number.MAX_SAFE_INTEGER,
    };
  }, [textDraft, source]);

  // 重绘：原图 → 标注（按 z_index 顺序），马赛克走真实像素化。
  useEffect(() => {
    const canvas = canvasRef.current;
    const image = imageRef.current;
    if (!canvas || !image || !source) return;
    const ctx = canvas.getContext("2d", { willReadFrequently: true });
    if (!ctx) return;

    ctx.clearRect(0, 0, canvas.width, canvas.height);
    ctx.drawImage(image, 0, 0, canvas.width, canvas.height);

    const all = [...items];
    if (liveAnnotation) all.push(liveAnnotation);
    if (draftText) all.push(draftText);
    all.sort((a, b) => a.z_index - b.z_index);
    for (const ann of all) {
      drawAnnotation(ctx, ann, source);
    }
  }, [items, liveAnnotation, draftText, source]);

  function imagePoint(event: React.PointerEvent): [number, number] | null {
    const canvas = canvasRef.current;
    if (!canvas || !source) return null;
    const rect = canvas.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) return null;
    const x = ((event.clientX - rect.left) * source.width) / rect.width;
    const y = ((event.clientY - rect.top) * source.height) / rect.height;
    return [
      Math.max(0, Math.min(source.width, x)),
      Math.max(0, Math.min(source.height, y)),
    ];
  }

  function handlePointerDown(event: React.PointerEvent) {
    const point = imagePoint(event);
    if (!point || !source) return;
    if (tool === "text") {
      setTextDraft({ x: point[0], y: point[1], value: "" });
      return;
    }
    dragStart.current = point;
    setDraftStroke([point[0], point[1]]);
    event.currentTarget.setPointerCapture(event.pointerId);
  }

  function handlePointerMove(event: React.PointerEvent) {
    if (!dragStart.current) return;
    const point = imagePoint(event);
    if (!point) return;
    setDraftStroke((previous) => {
      if (!previous) return previous;
      if (tool === "brush" || tool === "mosaic") return [...previous, point[0], point[1]];
      return [previous[0], previous[1], point[0], point[1]];
    });
  }

  function handlePointerUp(event: React.PointerEvent) {
    if (!dragStart.current || !source) return;
    const point = imagePoint(event) ?? dragStart.current;
    const stroke =
      draftStroke && draftStroke.length >= 4
        ? draftStroke
        : [dragStart.current[0], dragStart.current[1], point[0], point[1]];
    dragStart.current = null;
    setDraftStroke(null);
    const annotation = buildAnnotation(tool, stroke, source, crypto.randomUUID(), items.length);
    if (annotation) commit([...items, annotation]);
  }

  function commitTextDraft() {
    if (!textDraft || !source) {
      setTextDraft(null);
      return;
    }
    if (textDraft.value) {
      commit([
        ...items,
        {
          id: crypto.randomUUID(),
          kind: "text",
          geometry: [textDraft.x, textDraft.y],
          style: { text: textDraft.value, font_scale: fontScale(source) },
          z_index: items.length,
        },
      ]);
    }
    setTextDraft(null);
  }

  async function handleExport() {
    if (!source) return;
    try {
      setIsSaving(true);
      await invoke("export_annotated", { itemId, scene: { items } });
      success("标注已保存并复制到剪贴板");
      onDone();
    } catch (e) {
      showError(`保存失败：${e}`);
    } finally {
      setIsSaving(false);
    }
  }

  const displayScale = useMemo(() => {
    const canvas = canvasRef.current;
    if (!canvas || !source) return 1;
    const rect = canvas.getBoundingClientRect();
    return rect.width > 0 ? rect.width / source.width : 1;
  }, [source, items]);

  return (
    <div className="studio">
      <header className="studio-bar">
        <div className="row" style={{ gap: 7 }}>
          <EditIcon size={15} style={{ color: "var(--text-tertiary)" }} />
          <span style={{ fontSize: 13, fontWeight: 600 }}>标注</span>
        </div>

        <div className="studio-tools" role="group" aria-label="标注工具">
          {TOOLS.map((entry) => (
            <button
              key={entry.key}
              className="tool-btn"
              aria-pressed={tool === entry.key}
              title={entry.label}
              onClick={() => {
                commitTextDraft();
                setTool(entry.key);
              }}
            >
              {entry.icon}
            </button>
          ))}
        </div>

        <div className="studio-tools">
          <button
            className="tool-btn"
            title="撤销 (⌘Z)"
            disabled={undoStack.length === 0}
            onClick={undo}
          >
            <UndoIcon size={15} />
          </button>
          <button
            className="tool-btn"
            title="重做 (⌘⇧Z)"
            disabled={redoStack.length === 0}
            onClick={redo}
          >
            <RedoIcon size={15} />
          </button>
        </div>

        <div className="spacer" />

        {asciiWarning && (
          <span className="badge warning">导出字体仅支持 ASCII，非 ASCII 字符已忽略</span>
        )}

        <button className="btn" onClick={onDone}>
          <XIcon size={14} />
          <span>取消</span>
        </button>
        <button
          className="btn btn-primary"
          disabled={!source || isSaving}
          onClick={() => void handleExport()}
        >
          <CheckIcon size={14} />
          <span>{isSaving ? "生成中…" : "完成并复制"}</span>
        </button>
      </header>

      <div className="studio-stage">
        {!source && !loadError && <div className="empty-title">正在读取画面…</div>}

        {!source && loadError && (
          <div className="empty">
            <div className="empty-title" style={{ color: "var(--danger)" }}>
              画面加载失败
            </div>
            <div className="empty-sub">{loadError}</div>
            <button className="btn" onClick={() => void loadSource()}>
              重试
            </button>
          </div>
        )}

        {source && (
          <div className="studio-canvas">
            <canvas
              ref={canvasRef}
              width={source.width}
              height={source.height}
              onPointerDown={handlePointerDown}
              onPointerMove={handlePointerMove}
              onPointerUp={handlePointerUp}
            />
            {textDraft && (
              <input
                autoFocus
                className="input"
                value={textDraft.value}
                style={{
                  position: "absolute",
                  left: textDraft.x * displayScale,
                  top: textDraft.y * displayScale,
                  width: 200,
                  zIndex: 2,
                }}
                placeholder="输入文字后回车"
                onChange={(event) => {
                  const filtered = event.target.value.replace(/[^\x20-\x7E]/g, "");
                  setAsciiWarning(filtered !== event.target.value);
                  setTextDraft((draft) => (draft ? { ...draft, value: filtered } : draft));
                }}
                onKeyDown={(event) => {
                  if (event.key === "Enter") {
                    event.preventDefault();
                    commitTextDraft();
                  } else if (event.key === "Escape") {
                    event.preventDefault();
                    setTextDraft(null);
                  }
                }}
                onBlur={commitTextDraft}
              />
            )}
          </div>
        )}
      </div>
    </div>
  );
}

function buildAnnotation(
  tool: Tool,
  stroke: number[],
  source: Source,
  id: string,
  index: number
): Ann | null {
  const width = strokeWidth(source);
  const { block, radius } = mosaicMetrics(source);
  const [x0, y0] = stroke;
  const x1 = stroke[stroke.length - 2];
  const y1 = stroke[stroke.length - 1];

  if (tool === "brush") {
    if (stroke.length < 4) return null;
    return { id, kind: tool, geometry: stroke, style: { stroke_width: width }, z_index: index };
  }
  if (tool === "mosaic") {
    if (stroke.length < 2) return null;
    return {
      id,
      kind: tool,
      geometry: stroke,
      style: { block_size: block, brush_radius: radius },
      z_index: index,
    };
  }
  if (tool === "arrow") {
    if (Math.abs(x1 - x0) < 3 && Math.abs(y1 - y0) < 3) return null;
    return {
      id,
      kind: tool,
      geometry: [x0, y0, x1, y1],
      style: { stroke_width: width },
      z_index: index,
    };
  }
  if (tool === "text") return null;

  const w = Math.abs(x1 - x0);
  const h = Math.abs(y1 - y0);
  if (w < 2 || h < 2) return null;
  return {
    id,
    kind: tool,
    geometry: [Math.min(x0, x1), Math.min(y0, y1), w, h],
    style: { stroke_width: width },
    z_index: index,
  };
}

function drawAnnotation(ctx: CanvasRenderingContext2D, ann: Ann, source: Source) {
  const width = Number(ann.style.stroke_width ?? strokeWidth(source));
  ctx.save();
  ctx.strokeStyle = MARK_COLOR;
  ctx.fillStyle = MARK_COLOR;
  ctx.lineWidth = width;
  ctx.lineCap = "round";
  ctx.lineJoin = "round";

  switch (ann.kind) {
    case "rectangle": {
      const [x, y, w, h] = ann.geometry;
      ctx.strokeRect(x, y, w, h);
      break;
    }
    case "ellipse": {
      const [x, y, w, h] = ann.geometry;
      ctx.beginPath();
      ctx.ellipse(x + w / 2, y + h / 2, w / 2, h / 2, 0, 0, Math.PI * 2);
      ctx.stroke();
      break;
    }
    case "arrow": {
      const [x0, y0, x1, y1] = ann.geometry;
      ctx.beginPath();
      ctx.moveTo(x0, y0);
      ctx.lineTo(x1, y1);
      ctx.stroke();
      const angle = Math.atan2(y1 - y0, x1 - x0);
      const head = Math.max(10, width * 4);
      const spread = Math.PI / 6;
      ctx.beginPath();
      ctx.moveTo(x1, y1);
      ctx.lineTo(x1 - head * Math.cos(angle - spread), y1 - head * Math.sin(angle - spread));
      ctx.moveTo(x1, y1);
      ctx.lineTo(x1 - head * Math.cos(angle + spread), y1 - head * Math.sin(angle + spread));
      ctx.stroke();
      break;
    }
    case "brush": {
      ctx.beginPath();
      ctx.moveTo(ann.geometry[0], ann.geometry[1]);
      for (let i = 2; i + 1 < ann.geometry.length; i += 2) {
        ctx.lineTo(ann.geometry[i], ann.geometry[i + 1]);
      }
      ctx.stroke();
      break;
    }
    case "mosaic": {
      applyMosaic(ctx, ann, source);
      break;
    }
    case "text": {
      const [x, y] = ann.geometry;
      const scale = Number(ann.style.font_scale ?? fontScale(source));
      // 位图字体 7 像素高，导出侧按 font_scale 整数放大，这里用同样的字号预览。
      const size = 7 * scale;
      ctx.font = `600 ${size}px ${"ui-monospace, SFMono-Regular, Menlo, monospace"}`;
      ctx.textBaseline = "top";
      ctx.fillText(String(ann.style.text ?? ""), x, y);
      break;
    }
  }
  ctx.restore();
}

/**
 * 真实马赛克：格子对齐到图片原点，块内取平均色后整块填充，完全不透明。
 * 与 Rust `annotation::apply_mosaic` 同算法，所以预览等于导出结果。
 */
function applyMosaic(ctx: CanvasRenderingContext2D, ann: Ann, source: Source) {
  const block = Math.max(4, Math.min(96, Number(ann.style.block_size ?? mosaicMetrics(source).block)));
  const isBrush = ann.style.brush_radius !== undefined;
  const rects: Array<[number, number, number, number]> = [];

  if (!isBrush && ann.geometry.length === 4) {
    const [x, y, w, h] = ann.geometry;
    rects.push([x, y, w, h]);
  } else if (ann.geometry.length >= 2) {
    const radius = Math.max(1, Number(ann.style.brush_radius ?? block));
    const points: Array<[number, number]> = [];
    for (let i = 0; i + 1 < ann.geometry.length; i += 2) {
      points.push([ann.geometry[i], ann.geometry[i + 1]]);
    }
    if (points.length === 1) {
      const [cx, cy] = points[0];
      rects.push([cx - radius, cy - radius, radius * 2, radius * 2]);
    }
    for (let i = 0; i + 1 < points.length; i += 1) {
      const [ax, ay] = points[i];
      const [bx, by] = points[i + 1];
      const distance = Math.hypot(bx - ax, by - ay);
      const steps = Math.max(1, Math.ceil(distance / (block / 2)));
      for (let step = 0; step <= steps; step += 1) {
        const t = step / steps;
        const cx = ax + (bx - ax) * t;
        const cy = ay + (by - ay) * t;
        rects.push([cx - radius, cy - radius, radius * 2, radius * 2]);
      }
    }
  }
  if (rects.length === 0) return;

  const cells = new Set<string>();
  let minCellX = Infinity;
  let minCellY = Infinity;
  let maxCellX = -Infinity;
  let maxCellY = -Infinity;
  for (const [x, y, w, h] of rects) {
    if (w <= 0 || h <= 0) continue;
    for (let cy = Math.floor(y / block); cy < Math.ceil((y + h) / block); cy += 1) {
      for (let cx = Math.floor(x / block); cx < Math.ceil((x + w) / block); cx += 1) {
        cells.add(`${cx},${cy}`);
        minCellX = Math.min(minCellX, cx);
        minCellY = Math.min(minCellY, cy);
        maxCellX = Math.max(maxCellX, cx);
        maxCellY = Math.max(maxCellY, cy);
      }
    }
  }
  if (cells.size === 0) return;

  // 只读取覆盖范围内的像素，拖动时不必每帧复制整张画布。
  const readX = Math.max(0, minCellX * block);
  const readY = Math.max(0, minCellY * block);
  const readW = Math.min(source.width, (maxCellX + 1) * block) - readX;
  const readH = Math.min(source.height, (maxCellY + 1) * block) - readY;
  if (readW <= 0 || readH <= 0) return;
  const image = ctx.getImageData(readX, readY, readW, readH);

  for (const key of cells) {
    const [cx, cy] = key.split(",").map(Number);
    const x0 = Math.max(0, cx * block);
    const y0 = Math.max(0, cy * block);
    const x1 = Math.min(source.width, cx * block + block);
    const y1 = Math.min(source.height, cy * block + block);
    if (x0 >= x1 || y0 >= y1) continue;

    let r = 0;
    let g = 0;
    let b = 0;
    let count = 0;
    for (let y = y0; y < y1; y += 1) {
      for (let x = x0; x < x1; x += 1) {
        const index = ((y - readY) * readW + (x - readX)) * 4;
        r += image.data[index];
        g += image.data[index + 1];
        b += image.data[index + 2];
        count += 1;
      }
    }
    if (count === 0) continue;
    ctx.fillStyle = `rgb(${Math.round(r / count)} ${Math.round(g / count)} ${Math.round(b / count)})`;
    ctx.fillRect(x0, y0, x1 - x0, y1 - y0);
  }
}
