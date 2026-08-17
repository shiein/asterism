import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export function RecordingToolbar() {
  const params = useMemo(() => new URLSearchParams(window.location.search), []);
  const mode = params.get("mode") === "video" ? "video" : "gif";
  const startsAt = Number(params.get("startsAt")) || Date.now() + 3_000;
  const [now, setNow] = useState(Date.now());
  const [stopping, setStopping] = useState(false);
  const countdown = Math.max(0, Math.ceil((startsAt - now) / 1_000));
  const elapsedSeconds = Math.max(0, Math.floor((now - startsAt) / 1_000));

  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 100);
    return () => window.clearInterval(timer);
  }, []);

  useEffect(() => {
    function handleKeyDown(event: KeyboardEvent) {
      if ((event.key === "Escape" || event.key === " ") && countdown === 0 && !stopping) {
        event.preventDefault();
        void stop();
      }
    }
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [countdown, stopping]);

  async function stop() {
    if (stopping || countdown > 0) return;
    setStopping(true);
    try {
      const accepted = await invoke<boolean>("stop_recording");
      if (!accepted) setStopping(false);
    } catch {
      setStopping(false);
    }
  }

  return (
    <div className="rec-hud" data-tauri-drag-region>
      <div className="rec-hud-status" data-tauri-drag-region>
        <span className={`rec-live ${countdown > 0 ? "waiting" : ""}`} />
        <div data-tauri-drag-region>
          <strong data-tauri-drag-region>
            {countdown > 0 ? `${countdown} 秒后开始` : mode === "gif" ? "GIF 录制中" : "视频录制中"}
          </strong>
          <small data-tauri-drag-region>{mode === "gif" ? "12 FPS · GIF" : "30 FPS · H.264"}</small>
        </div>
      </div>
      <time dateTime={`PT${elapsedSeconds}S`} data-tauri-drag-region>
        {formatDuration(elapsedSeconds)}
      </time>
      <button
        className="rec-stop"
        disabled={countdown > 0 || stopping}
        onClick={() => void stop()}
        aria-label="停止录制"
      >
        <i />
        {stopping ? "保存中" : "停止"}
      </button>
    </div>
  );
}

function formatDuration(totalSeconds: number) {
  const hours = Math.floor(totalSeconds / 3_600);
  const minutes = Math.floor((totalSeconds % 3_600) / 60);
  const seconds = totalSeconds % 60;
  const short = `${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}`;
  return hours > 0 ? `${String(hours).padStart(2, "0")}:${short}` : short;
}
