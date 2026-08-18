import { useEffect, useState } from "react";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { copyItem, previewImage } from "../api";
import { CheckIcon, CopyIcon, XIcon } from "../components/icons";

export function PinWindowView() {
  const params = new URLSearchParams(window.location.search);
  const id = params.get("id");
  const [dataUrl, setDataUrl] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [scale, setScale] = useState(1);
  const [opacity, setOpacity] = useState(1);

  useEffect(() => {
    if (!id) return;
    previewImage(id)
      .then(setDataUrl)
      .catch((err) => console.error("failed to load pin image", err));
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

  function handleWheel(e: React.WheelEvent) {
    if (e.ctrlKey || e.metaKey) {
      // 调节透明度
      e.preventDefault();
      setOpacity((prev) => Math.min(1, Math.max(0.2, prev - e.deltaY * 0.001)));
    } else {
      // 缩放
      e.preventDefault();
      setScale((prev) => Math.min(3, Math.max(0.3, prev - e.deltaY * 0.0015)));
    }
  }

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
      className="pin-container"
      data-tauri-drag-region
      onWheel={handleWheel}
      onDoubleClick={handleClose}
      style={{ opacity }}
    >
      <div className="pin-toolbar" onClick={(e) => e.stopPropagation()}>
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
      ) : (
        <div className="pin-loading" data-tauri-drag-region>
          载入贴图…
        </div>
      )}
    </div>
  );
}
