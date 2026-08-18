import { useState, useRef, useEffect } from "react";
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
  XIcon,
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
  const mountedRef = useRef(true);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

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
      error("当前实例没有屏幕录制权限。授权后请完全退出并重新打开应用。");
      void permission.refetch();
      return;
    }
    if (detail.includes("microphone permission requested")) {
      error("已发起麦克风授权。允许后请重新开始视频录制。");
      return;
    }
    if (!detail.includes("cancelled")) {
      error(`${label}失败：${detail}`);
    }
  }

  async function runStillCapture(kind: "region" | "fullscreen" | "scroll") {
    const command =
      kind === "region"
        ? "capture_region"
        : kind === "fullscreen"
          ? "capture_fullscreen"
          : "scroll_capture";
    const label = kind === "region" ? "选区截图" : kind === "fullscreen" ? "全屏截图" : "滚动长图";
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
      if (mountedRef.current) {
        setActiveOperation(null);
      }
    }
  }

  async function startRecording(kind: "gif" | "video") {
    const label = kind === "gif" ? "GIF 录制" : "视频录制";
    try {
      setActiveOperation(kind);
      toast("主窗口将隐藏：拖选区域后倒计时 3 秒开始");
      if (kind === "gif") {
        await invoke("record_gif", { fps: 12 });
      } else {
        await invoke("record_video", { fps: 30, audio: audioMode });
      }
      await queryClient.invalidateQueries({ queryKey: ["history"] });
      success(kind === "gif" ? "GIF 已保存到历史" : "视频已保存到历史");
    } catch (cause) {
      reportCaptureError(label, cause);
    } finally {
      if (mountedRef.current) {
        setActiveOperation(null);
      }
    }
  }

  async function openPermissionSettings() {
    try {
      await invoke("open_screen_capture_settings");
      toast("请勾选「屏幕与系统音频录制」，然后完全退出并重新打开应用");
    } catch (cause) {
      error(`无法打开系统设置：${cause}`);
    }
  }

  return (
    <main className="pane">
      <header className="pane-header">
        <div className="pane-header-row">
          <div>
            <div className="pane-title">采集工作台</div>
            <div className="pane-subtitle">截图、滚动长图与屏幕录制</div>
          </div>
          <div className="flow" aria-label="录制流程">
            <span>隐藏窗口</span>
            <span>框选</span>
            <span>倒计时</span>
            <span>悬浮停止</span>
          </div>
        </div>
      </header>

      <div className="pane-body">
        <PermissionNotice
          status={permission.data}
          loading={permission.isLoading}
          onOpenSettings={() => void openPermissionSettings()}
          onRefresh={() => void permission.refetch()}
        />

        <section className="section">
          <div className="section-head">
            <h3>
              <CropIcon size={15} />
              截图
            </h3>
            <span className="badge">Esc 取消 · 右键回退</span>
          </div>
          <div className="section-body">
            <div className="capture-grid">
              <button
                className="capture-tile primary"
                disabled={busy}
                onClick={() => void runStillCapture("region")}
              >
                <span className="capture-tile-icon">
                  <CropIcon size={17} />
                </span>
                <span className="capture-tile-title">
                  {activeOperation === "region" ? "正在框选…" : "选区截图"}
                </span>
                <span className="capture-tile-desc">
                  直接在当前画面上框选，自动吸附窗口，框好即可标注
                </span>
                <span className="capture-tile-meta">工具栏支持矩形 / 箭头 / 马赛克 / 文字</span>
              </button>

              <button
                className="capture-tile"
                disabled={busy}
                onClick={() => void runStillCapture("fullscreen")}
              >
                <span className="capture-tile-icon">
                  <MaximizeIcon size={17} />
                </span>
                <span className="capture-tile-title">
                  {activeOperation === "fullscreen" ? "正在截图…" : "全屏截图"}
                </span>
                <span className="capture-tile-desc">捕获光标所在的整块屏幕</span>
                <span className="capture-tile-meta">自动写入剪贴板与历史</span>
              </button>

              <button
                className="capture-tile"
                disabled={busy}
                onClick={() => void runStillCapture("scroll")}
              >
                <span className="capture-tile-icon">
                  <ScrollIcon size={17} />
                </span>
                <span className="capture-tile-title">
                  {activeOperation === "scroll" ? "正在滚动拼接…" : "滚动长图"}
                </span>
                <span className="capture-tile-desc">框选后自动滚动，按重叠区域拼接</span>
                <span className="capture-tile-meta">匹配置信度过低会提前停止</span>
              </button>
            </div>
          </div>
        </section>

        <section className="section">
          <div className="section-head">
            <h3>
              <VideoIcon size={15} />
              录制
            </h3>
            <span className="badge">悬浮控制条不会进入画面</span>
          </div>
          <div className="section-body">
            <div className="capture-grid">
              <button
                className="capture-tile"
                disabled={busy}
                onClick={() => void startRecording("gif")}
              >
                <span className="capture-tile-icon">
                  <PlayIcon size={16} />
                </span>
                <span className="capture-tile-title">
                  {activeOperation === "gif" ? "GIF 录制中…" : "录制 GIF"}
                </span>
                <span className="capture-tile-desc">12 FPS，录到你点击停止为止</span>
                <span className="capture-tile-meta">
                  <i className="record-dot" />
                  无时长上限
                </span>
              </button>

              <div className="capture-tile" style={{ cursor: "default" }}>
                <span className="capture-tile-icon">
                  <VideoIcon size={17} />
                </span>
                <span className="capture-tile-title">录制视频</span>
                <span className="capture-tile-desc">
                  {isMac
                    ? "H.264 硬件编码 MP4，30 FPS，可选录入声音"
                    : "当前平台的 H.264 原生编码尚未完成，不会用低清格式冒充"}
                </span>
                <div className="row" style={{ width: "100%", marginTop: "auto" }}>
                  <select
                    className="select"
                    style={{ maxWidth: 132 }}
                    value={audioMode}
                    onChange={(event) => setAudioMode(event.target.value as AudioMode)}
                    disabled={busy || !isMac}
                    aria-label="音频来源"
                  >
                    <option value="none">无音频</option>
                    <option value="system">系统声音</option>
                    <option value="mic">麦克风</option>
                    <option value="both">系统 + 麦克风</option>
                  </select>
                  <button
                    className="btn btn-primary"
                    disabled={busy || !isMac}
                    onClick={() => void startRecording("video")}
                  >
                    {activeOperation === "video" ? "录制中…" : isMac ? "开始" : "不可用"}
                  </button>
                </div>
              </div>
            </div>
          </div>
        </section>
      </div>
    </main>
  );
}

function PermissionNotice({
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
    return (
      <div className="notice" style={{ marginBottom: 14 }}>
        <ShieldCheckIcon size={15} className="notice-icon" />
        <div>正在检测屏幕捕获权限…</div>
      </div>
    );
  }
  if (status.granted) {
    return (
      <div className="notice success" style={{ marginBottom: 14 }}>
        <ShieldCheckIcon size={15} className="notice-icon" />
        <div>
          <strong>屏幕捕获已授权</strong>
          当前进程 {status.processName}
        </div>
      </div>
    );
  }
  return (
    <div className="notice danger" style={{ marginBottom: 14, flexDirection: "column" }}>
      <div className="row" style={{ alignItems: "flex-start" }}>
        <XIcon size={15} className="notice-icon" />
        <div>
          <strong>当前运行实例未获授权</strong>
          系统权限按应用身份绑定：列表里存在旧的 Asterism，不代表当前进程已获授权。
          <div style={{ marginTop: 4, fontFamily: "var(--font-mono)", fontSize: 11 }}>
            {status.bundleId} · {status.processName}
          </div>
        </div>
      </div>
      <div className="row" style={{ marginTop: 10 }}>
        {status.settingsAvailable && (
          <button className="btn" onClick={onOpenSettings}>
            打开系统设置
          </button>
        )}
        <button className="btn btn-plain" onClick={onRefresh}>
          重新检测
        </button>
      </div>
    </div>
  );
}
