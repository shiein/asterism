import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { AnnotatePage } from "./capture/AnnotatePage";
import { HistoryPage } from "./history/HistoryPage";
import { SettingsPage } from "./settings/SettingsPage";

export function App() {
  const [tab, setTab] = useState<"history" | "settings" | "capture">("history");
  const [annotate, setAnnotate] = useState<string | null>(null);

  return (
    <div>
      <nav className="filters" style={{ maxWidth: 860, margin: "16px auto 0", padding: "0 20px" }}>
        <button className={tab === "history" ? "chip on" : "chip"} onClick={() => setTab("history")}>
          历史
        </button>
        <button className={tab === "capture" ? "chip on" : "chip"} onClick={() => setTab("capture")}>
          采集
        </button>
        <button className={tab === "settings" ? "chip on" : "chip"} onClick={() => setTab("settings")}>
          设置
        </button>
      </nav>
      {tab === "history" && <HistoryPage />}
      {tab === "settings" && (
        <div className="app">
          <SettingsPage />
        </div>
      )}
      {tab === "capture" && (
        <div className="app">
          <div className="actions">
            <button onClick={() => void invoke("record_gif", { seconds: 3, fps: 10 })}>录 GIF 3s</button>
            <button onClick={() => void invoke("record_video", { seconds: 3, fps: 15 })}>录视频 3s</button>
            <button onClick={() => void invoke("scroll_capture", { frames: 8 })}>滚动截图</button>
            <button
              onClick={() =>
                void invoke<string>("capture_region").then((id) => setAnnotate(id))
              }
            >
              选区并标注
            </button>
          </div>
          {annotate && <AnnotatePage itemId={annotate} onDone={() => setAnnotate(null)} />}
        </div>
      )}
    </div>
  );
}
