import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useToast } from "../components/Toast";
import {
  CropIcon,
  MaximizeIcon,
  ScrollIcon,
  VideoIcon,
  PlayIcon,
  ShieldCheckIcon,
} from "../components/icons";

interface CaptureStudioPageProps {
  onAnnotate: (id: string) => void;
}

interface CapturePermissionStatus {
  granted: boolean;
  processName: string;
  bundleId: string;
  settingsAvailable: boolean;
  restartRecommendedAfterGrant: boolean;
}

type ActiveOperation = "region" | "fullscreen" | "scroll" | "gif" | "video" | null;
type AudioMode = "none" | "system" | "mic" | "both";

export function CaptureStudioPage({ onAnnotate }: CaptureStudioPageProps) {
  const queryClient = useQueryClient();
  const { success, error, toast } = useToast();
  const [activeOperation, setActiveOperation] = useState<ActiveOperation>(null);
  const [audioMode, setAudioMode] = useState<AudioMode>("none");
  const isMac = typeof navigator !== "undefined" && navigator.userAgent.includes("Macintosh");
  const permission = useQuery({
    queryKey: ["capture-permission"],
    queryFn: () => invoke<CapturePermissionStatus>("capture_permission_status"),
    refetchOnWindowFocus: true,
  });
  const busy = activeOperation !== null;

  function reportCaptureError(label: string, cause: unknown) {
    const detail = String(cause);
    if (detail.includes("screen capture permission denied")) {
      error("当前运行的 Asterism 尚未获得屏幕录制权限。授权后请完全退出并重新打开应用，再点击重试。");
      void permission.refetch();
      return;
    }
    if (detail.includes("microphone permission requested")) {
      error("已发起麦克风授权。允许访问后，请再次开始视频录制。");
      return;
    }
    if (!detail.includes("cancelled")) {
      error(`${label}失败：${detail}`);
    }
  }

  async function runStillCapture(kind: Exclude<ActiveOperation, "gif" | "video" | null>) {
    const command = kind === "region" ? "capture_region" : kind === "fullscreen" ? "capture_fullscreen" : "scroll_capture";
    const label = kind === "region" ? "选区截图" : kind === "fullscreen" ? "全屏截图" : "滚动截图";
    try {
      setActiveOperation(kind);
      const id = await invoke<string>(command, kind === "scroll" ? { frames: 8 } : undefined);
      await queryClient.invalidateQueries({ queryKey: ["history"] });
      success(`${label}完成`);
      if (kind !== "scroll") {
        onAnnotate(id);
      }
    } catch (cause) {
      reportCaptureError(label, cause);
    } finally {
      setActiveOperation(null);
    }
  }

  async function startRecording(kind: "gif" | "video") {
    const label = kind === "gif" ? "GIF 录制" : "视频录制";
    if (kind === "video" && !isMac) {
      toast("当前平台尚未提供 H.264 高清录制，未创建降级文件");
      return;
    }
    try {
      setActiveOperation(kind);
      toast("主窗口将隐藏：拖选区域后，倒计时 3 秒开始录制");
      if (kind === "gif") {
        await invoke("record_gif", { fps: 12 });
      } else {
        await invoke("record_video", { fps: 30, audio: audioMode });
      }
      await queryClient.invalidateQueries({ queryKey: ["history"] });
      success(kind === "gif" ? "GIF 已生成并保存到历史" : "H.264 视频已保存到历史");
    } catch (cause) {
      reportCaptureError(label, cause);
    } finally {
      setActiveOperation(null);
    }
  }

  async function openPermissionSettings() {
    try {
      await invoke("open_screen_capture_settings");
      toast("请为当前 Asterism 打开“屏幕与系统音频录制”，然后完全退出并重新打开应用");
    } catch (cause) {
      error(`无法打开系统设置：${cause}`);
    }
  }

  return (
    <main className="main-content capture-studio">
      <div className="capture-studio-scroll">
        <header className="capture-hero">
          <div className="capture-hero-copy">
            <div className="capture-eyebrow"><span /> CAPTURE SUITE</div>
            <h1>先让工具消失，<br />再留下画面。</h1>
            <p>截图、GIF 与视频使用同一套采集流程：隐藏主窗口、选择区域、开始采集，完成后自动恢复。</p>
          </div>
          <PermissionPanel
            status={permission.data}
            loading={permission.isLoading}
            onOpenSettings={() => void openPermissionSettings()}
            onRefresh={() => void permission.refetch()}
          />
        </header>

        <section className="capture-primary-grid" aria-label="截图方式">
          <button
            className="capture-primary-card"
            disabled={busy}
            onClick={() => void runStillCapture("region")}
          >
            <div className="capture-focus-mark" aria-hidden="true">
              <span className="corner corner-tl" /><span className="corner corner-tr" />
              <span className="corner corner-bl" /><span className="corner corner-br" />
              <span className="focus-star">✦</span>
            </div>
            <div className="capture-primary-copy">
              <span className="capture-card-kicker">PRIMARY</span>
              <h2>{activeOperation === "region" ? "正在唤起选区…" : "选区截图"}</h2>
              <p>自动隐藏 Asterism，冻结当前屏幕后精确拖选，完成即进入标注。</p>
              <span className="capture-card-action"><CropIcon size={16} /> 开始截图</span>
            </div>
          </button>

          <div className="capture-quick-stack">
            <button className="capture-quick-card" disabled={busy} onClick={() => void runStillCapture("fullscreen")}>
              <span className="capture-quick-icon cyan"><MaximizeIcon size={21} /></span>
              <span><strong>全屏截图</strong><small>隐藏窗口后捕获光标所在屏幕</small></span>
              <span className="capture-arrow">↗</span>
            </button>
            <button className="capture-quick-card" disabled={busy} onClick={() => void runStillCapture("scroll")}>
              <span className="capture-quick-icon amber"><ScrollIcon size={21} /></span>
              <span><strong>滚动长图</strong><small>选区后自动滚动并智能拼接</small></span>
              <span className="capture-arrow">↗</span>
            </button>
          </div>
        </section>

        <section className="recording-section">
          <div className="capture-section-heading">
            <div>
              <span className="capture-card-kicker">RECORD</span>
              <h2>动态录制</h2>
            </div>
            <div className="capture-flow-pills" aria-label="录制流程">
              <span>窗口隐藏</span><i>→</i><span>选区</span><i>→</i><span>3 秒倒计时</span><i>→</i><span>悬浮停止</span>
            </div>
          </div>

          <div className="recording-grid">
            <article className="recording-card gif-card">
              <div className="recording-card-top">
                <span className="recording-format-icon"><PlayIcon size={17} /></span>
                <span className="recording-format">GIF · 12 FPS</span>
              </div>
              <h3>录到你点击停止</h3>
              <p>不再预设 3 秒或 5 秒上限。浮动控制条受系统防捕获保护，不会进入最终画面。</p>
              <button className="recording-start-button" disabled={busy} onClick={() => void startRecording("gif")}>
                <span className="record-dot" />
                {activeOperation === "gif" ? "录制进行中" : "开始 GIF 录制"}
              </button>
            </article>

            <article className="recording-card video-card">
              <div className="recording-card-top">
                <span className="recording-format-icon"><VideoIcon size={18} /></span>
                <span className="recording-format">H.264 · 原始选区 · 30 FPS</span>
              </div>
              <h3>高清视频，同一套顺手流程</h3>
              <p>{isMac ? "硬件编码 MP4；时长由你控制。可按场景选择是否录入声音。" : "当前平台的 H.264 原生编码尚未完成，因此不会用低清格式伪装成功。"}</p>
              <div className="recording-controls">
                <label>
                  <span>音频</span>
                  <select value={audioMode} onChange={(event) => setAudioMode(event.target.value as AudioMode)} disabled={busy || !isMac}>
                    <option value="none">无音频</option>
                    <option value="system">系统声音</option>
                    <option value="mic">麦克风</option>
                    <option value="both">系统 + 麦克风</option>
                  </select>
                </label>
                <button className="recording-start-button" disabled={busy || !isMac} onClick={() => void startRecording("video")}>
                  <span className="record-dot" />
                  {activeOperation === "video" ? "录制进行中" : isMac ? "开始视频录制" : "当前平台不可用"}
                </button>
              </div>
            </article>
          </div>
        </section>
      </div>
    </main>
  );
}

function PermissionPanel({
  status,
  loading,
  onOpenSettings,
  onRefresh,
}: {
  status?: CapturePermissionStatus;
  loading: boolean;
  onOpenSettings: () => void;
  onRefresh: () => void;
}) {
  if (loading || !status) {
    return <div className="permission-panel checking"><span className="permission-pulse" />正在检测屏幕捕获权限…</div>;
  }
  if (status.granted) {
    return (
      <div className="permission-panel granted">
        <ShieldCheckIcon size={20} />
        <div><strong>屏幕捕获已授权</strong><small>当前进程 · {status.processName}</small></div>
      </div>
    );
  }
  return (
    <div className="permission-panel denied">
      <div className="permission-panel-copy">
        <span className="permission-alert">!</span>
        <div>
          <strong>当前运行实例未获授权</strong>
          <small>系统权限按应用身份绑定；列表里已有旧 Asterism，也不代表当前进程已获授权。</small>
          <code>{status.bundleId} · {status.processName}</code>
        </div>
      </div>
      <div className="permission-actions">
        {status.settingsAvailable && <button onClick={onOpenSettings}>打开系统设置</button>}
        <button className="permission-refresh" onClick={onRefresh}>重新检测</button>
      </div>
    </div>
  );
}
