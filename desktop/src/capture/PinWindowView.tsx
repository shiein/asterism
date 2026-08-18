import { useEffect, useRef, useState } from "react";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { copyItem, previewImage } from "../api";
import { CheckIcon, CopyIcon, XIcon } from "../components/icons";

export function PinWindowView() {
  const params = new URLSearchParams(window.location.search);
  const id = params.get("id");
  const containerRef = useRef<HTMLDivElement>(null);
  const [dataUrl, setDataUrl] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [scale, setScale] = useState(1);
  const [opacity, setOpacity] = useState(1);

  useEffect(() => {
    if (!id) {
      setLoadError("未指定图片 ID");
      setLoading(false);
      return;
    }
    setLoading(true);
    setLoadError(null);
    previewImage(id)
      .then((url) => {
        setDataUrl(url);
        setLoading(false);
      })
      .catch((err) => {
        console.error("failed to load pin image", err);
        setLoadError(String(err));
        setLoading(false);
      });
  }, [id]);

  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      if (e.key === "Escape") {
        void getCurrentWebviewWindow().close();
      }
    }
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, []);

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;

    function handleWheel(e: WheelEvent) {
      e.preventDefault();
      if (e.ctrlKey || e.metaKey) {
        setOpacity((prev) => Math.min(1, Math.max(0.2, prev - e.deltaY * 0.001)));
      } else {
        setScale((prev) => Math.min(3, Math.max(0.3, prev - e.deltaY * 0.0015)));
      }
    }

    el.addEventListener("wheel", handleWheel, { passive: false });
    return () => el.removeEventListener("wheel", handleWheel);
  }, []);

  async function handleCopy() {
    if (!id) return;
    try {
      await copyItem(id);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch (err) {
      console.error("failed to copy pinned image", err);
    }
  }

  function handleClose() {
    void getCurrentWebviewWindow().close();
  }

  return (
    <div
      ref={containerRef}
      className="pin-container"
      data-tauri-drag-region
      onDoubleClick={handleClose}
      style={{ opacity }}
    >
      <div
        className="pin-toolbar"
        data-tauri-drag-region="false"
        onClick={(e) => e.stopPropagation()}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <button
          className="pin-btn"
          onClick={handleCopy}
          title={copied ? "已复制" : "复制到剪贴板"}
        >
          {copied ? <CheckIcon size={13} /> : <CopyIcon size={13} />}
        </button>
        <button
          className="pin-btn"
          onClick={() => {
            setScale(1);
            setOpacity(1);
          }}
          title="重置缩放/透明度"
        >
          {Math.round(scale * 100)}%
        </button>
        <button className="pin-btn pin-close" onClick={handleClose} title="关闭 (ESC / 双击)">
          <XIcon size={13} />
        </button>
      </div>

      {dataUrl ? (
        <img
          src={dataUrl}
          alt="贴图"
          className="pin-image"
          data-tauri-drag-region
          style={{ transform: `scale(${scale})` }}
          draggable={false}
        />
      ) : loading ? (
        <div className="pin-loading" data-tauri-drag-region>
          载入贴图…
        </div>
      ) : (
        <div className="pin-error" data-tauri-drag-region>
          <span>贴图载入失败</span>
          <small>{loadError}</small>
          <button className="btn btn-plain" onClick={handleClose} style={{ marginTop: 8 }}>
            关闭
          </button>
        </div>
      )}
    </div>
  );
}
