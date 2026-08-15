import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { AnnotatePage } from "./capture/AnnotatePage";
import { HistoryPage } from "./history/HistoryPage";
import { SettingsPage } from "./settings/SettingsPage";

export function App() {
  const queryClient = useQueryClient();
  const [tab, setTab] = useState<"history" | "settings" | "capture">("history");
  const [annotate, setAnnotate] = useState<string | null>(null);

  function closeAnnotate() {
    setAnnotate(null);
    void queryClient.invalidateQueries({ queryKey: ["history"] });
  }

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
      {annotate && (
        <div className="app">
          <AnnotatePage itemId={annotate} onDone={closeAnnotate} />
        </div>
      )}
      {tab === "history" && <HistoryPage onAnnotate={setAnnotate} />}
      {tab === "settings" && (
        <div className="app">
          <SettingsPage />
        </div>
      )}
      {tab === "capture" && (
        <div className="app">
          <div className="actions">
            <button onClick={() => void invoke("record_gif", { seconds: 3, fps: 10 })}>录 GIF 3s</button>
            <button
              disabled={!navigator.userAgent.includes("Macintosh")}
              title={navigator.userAgent.includes("Macintosh") ? undefined : "当前平台的视频音频录制尚未实现"}
              onClick={() => void invoke("record_video", { seconds: 3, fps: 30, audio: "both" })}
            >
              {navigator.userAgent.includes("Macintosh") ? "录 H.264（系统音+麦）" : "视频录制（当前平台不可用）"}
            </button>
            <button onClick={() => void invoke("scroll_capture", { frames: 8 })}>滚动截图</button>
            <button
              onClick={() =>
                void invoke<string>("capture_region").then((id) => setAnnotate(id))
              }
            >
              选区并标注
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
