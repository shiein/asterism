import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useQueryClient } from "@tanstack/react-query";
import { useToast } from "../components/Toast";
import {
  CropIcon,
  MaximizeIcon,
  ScrollIcon,
  VideoIcon,
  CameraIcon,
  PlayIcon,
} from "../components/icons";

interface CaptureStudioPageProps {
  onAnnotate: (id: string) => void;
}

export function CaptureStudioPage({ onAnnotate }: CaptureStudioPageProps) {
  const queryClient = useQueryClient();
  const { success, error, toast } = useToast();
  const [isRecordingGif, setIsRecordingGif] = useState(false);
  const [isRecordingVideo, setIsRecordingVideo] = useState(false);

  const isMac = typeof navigator !== "undefined" && navigator.userAgent.includes("Macintosh");

  async function handleCaptureRegion() {
    try {
      const id = await invoke<string>("capture_region");
      void queryClient.invalidateQueries({ queryKey: ["history"] });
      success("选区截图完成");
      onAnnotate(id);
    } catch (e) {
      if (!String(e).includes("cancelled")) {
        error(`选区截图失败: ${e}`);
      }
    }
  }

  async function handleCaptureFullscreen() {
    try {
      const id = await invoke<string>("capture_fullscreen");
      void queryClient.invalidateQueries({ queryKey: ["history"] });
      success("全屏截图完成，已存入剪贴板");
      onAnnotate(id);
    } catch (e) {
      if (!String(e).includes("cancelled")) {
        error(`全屏截图失败: ${e}`);
      }
    }
  }

  async function handleScrollCapture() {
    try {
      toast("正在执行 8 帧滚动截图拼接…");
      await invoke("scroll_capture", { frames: 8 });
      void queryClient.invalidateQueries({ queryKey: ["history"] });
      success("滚动截图已保存");
    } catch (e) {
      error(`滚动截图失败: ${e}`);
    }
  }

  async function handleRecordGif(seconds: number = 3) {
    try {
      setIsRecordingGif(true);
      toast(`正在录制 ${seconds} 秒 GIF 动图…`);
      await invoke("record_gif", { seconds, fps: 10 });
      void queryClient.invalidateQueries({ queryKey: ["history"] });
      success("GIF 动图已生成并保存到历史");
    } catch (e) {
      error(`GIF 录制失败: ${e}`);
    } finally {
      setIsRecordingGif(false);
    }
  }

  async function handleRecordVideo(seconds: number = 3) {
    if (!isMac) {
      toast("当前平台的视频音频录制能力尚在开发中");
      return;
    }
    try {
      setIsRecordingVideo(true);
      toast(`正在录制 ${seconds} 秒高清视频（含麦克风与系统音频）…`);
      await invoke("record_video", { seconds, fps: 30, audio: "both" });
      void queryClient.invalidateQueries({ queryKey: ["history"] });
      success("H.264 视频录制完成");
    } catch (e) {
      error(`视频录制失败: ${e}`);
    } finally {
      setIsRecordingVideo(false);
    }
  }

  return (
    <main className="main-content">
      <header className="page-header" style={{ padding: "20px 28px" }}>
        <div>
          <h2 style={{ fontSize: 20, fontWeight: 700, letterSpacing: -0.02em }}>采集工作台 Studio</h2>
          <p style={{ fontSize: 13, color: "var(--text-secondary)", marginTop: 2 }}>
            高精度屏幕截取、长图滚动拼接与多媒体录制套件
          </p>
        </div>
      </header>

      <div style={{ flex: 1, overflowY: "auto", padding: "24px 28px" }}>
        <div className="studio-grid">
          {/* Region Capture */}
          <div className="studio-card">
            <div>
              <div className="studio-card-icon">
                <CropIcon size={22} />
              </div>
              <div style={{ marginTop: 14 }}>
                <div className="studio-card-title">选区截图并标注</div>
                <div className="studio-card-desc">
                  自由拖选屏幕区域，支持放大镜像素精确定位，截取后直接进入矢量标注画布。
                </div>
              </div>
            </div>
            <button className="btn btn-primary" onClick={handleCaptureRegion} style={{ width: "100%" }}>
              <CropIcon size={15} />
              <span>开始选区截屏</span>
            </button>
          </div>

          {/* Fullscreen Capture */}
          <div className="studio-card">
            <div>
              <div className="studio-card-icon" style={{ background: "rgba(14, 165, 233, 0.12)", color: "#38bdf8" }}>
                <MaximizeIcon size={22} />
              </div>
              <div style={{ marginTop: 14 }}>
                <div className="studio-card-title">全屏即时截图</div>
                <div className="studio-card-desc">
                  一键捕捉当前主显示器完整全屏，自动复制至系统剪贴板并同步至历史记录。
                </div>
              </div>
            </div>
            <button className="btn btn-secondary" onClick={handleCaptureFullscreen} style={{ width: "100%" }}>
              <MaximizeIcon size={15} />
              <span>截取全屏画面</span>
            </button>
          </div>

          {/* Scroll Capture */}
          <div className="studio-card">
            <div>
              <div className="studio-card-icon" style={{ background: "rgba(245, 158, 11, 0.12)", color: "#fbbf24" }}>
                <ScrollIcon size={22} />
              </div>
              <div style={{ marginTop: 14 }}>
                <div className="studio-card-title">长图滚动截屏</div>
                <div className="studio-card-desc">
                  自动连续向下滚动并捕获页面内容，利用图像特征平滑缝合为一张高清完整长截图。
                </div>
              </div>
            </div>
            <button className="btn btn-secondary" onClick={handleScrollCapture} style={{ width: "100%" }}>
              <ScrollIcon size={15} />
              <span>长图滚动截图</span>
            </button>
          </div>

          {/* GIF Recording */}
          <div className="studio-card">
            <div>
              <div className="studio-card-icon" style={{ background: "rgba(168, 85, 247, 0.12)", color: "#c084fc" }}>
                <PlayIcon size={22} />
              </div>
              <div style={{ marginTop: 14 }}>
                <div className="studio-card-title">GIF 动图录制</div>
                <div className="studio-card-desc">
                  轻量级录制短时动态操作，自动压缩生成标准 GIF 动图，便于快速分享与演示。
                </div>
              </div>
            </div>
            <div style={{ display: "flex", gap: 8 }}>
              <button
                className="btn btn-secondary"
                disabled={isRecordingGif}
                onClick={() => void handleRecordGif(3)}
                style={{ flex: 1 }}
              >
                <span>{isRecordingGif ? "录制中…" : "录制 3 秒"}</span>
              </button>
              <button
                className="btn btn-secondary"
                disabled={isRecordingGif}
                onClick={() => void handleRecordGif(5)}
                style={{ flex: 1 }}
              >
                <span>{isRecordingGif ? "录制中…" : "录制 5 秒"}</span>
              </button>
            </div>
          </div>

          {/* Video Recording */}
          <div className="studio-card">
            <div>
              <div className="studio-card-icon" style={{ background: "rgba(239, 68, 68, 0.12)", color: "#f87171" }}>
                <VideoIcon size={22} />
              </div>
              <div style={{ marginTop: 14 }}>
                <div className="studio-card-title">高清视频录像</div>
                <div className="studio-card-desc">
                  {isMac
                    ? "基于 macOS AVFoundation 硬件加速录制 H.264 视频，同步采集麦克风与系统内录音频。"
                    : "原生音视频录屏引擎（macOS 优先，其他平台即将支持）。"}
                </div>
              </div>
            </div>
            <button
              className="btn btn-secondary"
              disabled={isRecordingVideo || !isMac}
              onClick={() => void handleRecordVideo(3)}
              style={{ width: "100%" }}
            >
              <VideoIcon size={15} />
              <span>{isRecordingVideo ? "录制视频中…" : isMac ? "录制 3 秒 H.264" : "当前平台不可用"}</span>
            </button>
          </div>
        </div>
      </div>
    </main>
  );
}
