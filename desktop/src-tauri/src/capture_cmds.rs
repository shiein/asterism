use asterism_capture::backend::CaptureBackend;
use asterism_capture::{AnnotationScene, OverlaySession, XcapBackend, export_png};
use asterism_core::ContentKind;
use asterism_media::AudioSource;
use asterism_media::VideoFrame;
#[cfg(not(target_os = "macos"))]
use asterism_media::avi::AviMjpeg;
use asterism_media::gifenc::GifSession;
#[cfg(target_os = "macos")]
use asterism_media::macos::MacOsRecording;
use asterism_media::video::RecordingPlan;
use base64::Engine;
use tauri::State;

use crate::commands::{CmdError, insert_screenshot};
use crate::runtime::DesktopState;

#[derive(serde::Serialize)]
pub struct AnnotationSource {
    data_url: String,
    width: u32,
    height: u32,
}

#[tauri::command]
pub fn annotation_source(
    state: State<'_, DesktopState>,
    item_id: String,
) -> Result<AnnotationSource, CmdError> {
    let (_, png, width, height) = load_image_item(&state, &item_id)?;
    Ok(AnnotationSource {
        data_url: format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(png)
        ),
        width,
        height,
    })
}

#[tauri::command]
pub fn list_windows() -> Result<Vec<asterism_capture::WindowInfo>, CmdError> {
    XcapBackend.list_windows().map_err(|e| CmdError::Any(e.to_string()))
}

#[tauri::command]
pub fn capture_window(state: State<'_, DesktopState>, id: u32) -> Result<String, CmdError> {
    let frame = XcapBackend.capture_window(id).map_err(|e| CmdError::Any(e.to_string()))?;
    let png = export_png(frame.width, frame.height, &frame.bgra, &AnnotationScene::default())
        .map_err(CmdError::Any)?;
    insert_screenshot(&state, png, frame.width, frame.height)
}

#[tauri::command]
pub fn export_annotated(
    state: State<'_, DesktopState>,
    item_id: String,
    scene: AnnotationScene,
) -> Result<String, CmdError> {
    let (_, png, width, height) = load_image_item(&state, &item_id)?;
    let img = image::load_from_memory(&png).map_err(|e| CmdError::Any(e.to_string()))?;
    let rgba = img.to_rgba8();
    let mut bgra = vec![0u8; rgba.len()];
    for (src, dst) in rgba.chunks_exact(4).zip(bgra.chunks_exact_mut(4)) {
        dst[0] = src[2];
        dst[1] = src[1];
        dst[2] = src[0];
        dst[3] = src[3];
    }
    let out = export_png(width, height, &bgra, &scene).map_err(CmdError::Any)?;
    insert_screenshot(&state, out, width, height)
}

fn load_image_item(
    state: &DesktopState,
    item_id: &str,
) -> Result<(asterism_core::ContentId, Vec<u8>, u32, u32), CmdError> {
    let id = item_id.parse().map_err(|e: asterism_core::CoreError| CmdError::Any(e.to_string()))?;
    let item = state.store.get(id).map_err(|e| CmdError::Any(e.to_string()))?;
    let png = match item.payload_ref {
        asterism_core::PayloadRef::Blob { blob_id } => {
            state.store.get_blob(&blob_id).map_err(|e| CmdError::Any(e.to_string()))?
        }
        asterism_core::PayloadRef::Inline { bytes } => bytes.to_vec(),
        _ => return Err(CmdError::Any("not an image".into())),
    };
    let img = image::load_from_memory(&png).map_err(|e| CmdError::Any(e.to_string()))?;
    Ok((id, png, img.width(), img.height()))
}

#[tauri::command]
pub async fn record_gif(
    state: State<'_, DesktopState>,
    seconds: u32,
    fps: u16,
) -> Result<String, CmdError> {
    let (bytes, w, h) =
        tauri::async_runtime::spawn_blocking(move || record_gif_inner(seconds, fps))
            .await
            .map_err(|e| CmdError::Any(e.to_string()))??;
    insert_blob(&state, bytes, w, h, ContentKind::Gif)
}

fn record_gif_inner(seconds: u32, fps: u16) -> Result<(Vec<u8>, u32, u32), CmdError> {
    let backend = XcapBackend;
    let monitors = backend.list_monitors().map_err(|e| CmdError::Any(e.to_string()))?;
    let monitor = asterism_capture::preferred_monitor(&monitors)
        .ok_or_else(|| CmdError::Any("no monitor".into()))?;
    let first = backend.capture_display(monitor).map_err(|e| CmdError::Any(e.to_string()))?;
    let sel = crate::overlay_cli::select_region_subprocess(&first)
        .map_err(|e| CmdError::Any(e.to_string()))?;
    let session = OverlaySession { frame: first.clone(), selection: sel };
    let (w, h, _) = session.crop_bgra().ok_or_else(|| CmdError::Any("need selection".into()))?;
    let mut gif = GifSession::new(w, h, fps);
    let frames = (seconds.max(1) * u32::from(fps.clamp(8, 15))).min(150);
    for _ in 0..frames {
        let frame = backend.capture_display(monitor).map_err(|e| CmdError::Any(e.to_string()))?;
        let sess = OverlaySession { frame, selection: session.selection.clone() };
        if let Some((cw, ch, bgra)) = sess.crop_bgra() {
            let vf = VideoFrame { timestamp_us: 0, width: cw, height: ch, bgra };
            let _ = gif.push(&vf);
        }
        std::thread::sleep(std::time::Duration::from_millis(1000 / u64::from(fps.max(8))));
    }
    let bytes = gif.finish().map_err(|e| CmdError::Any(e.to_string()))?;
    Ok((bytes, w, h))
}

#[tauri::command]
pub async fn record_video(
    state: State<'_, DesktopState>,
    seconds: u32,
    fps: u32,
    audio: Option<String>,
) -> Result<String, CmdError> {
    let (bytes, w, h) =
        tauri::async_runtime::spawn_blocking(move || record_video_inner(seconds, fps, audio))
            .await
            .map_err(|e| CmdError::Any(e.to_string()))??;
    insert_blob(&state, bytes, w, h, ContentKind::Video)
}

fn record_video_inner(
    seconds: u32,
    fps: u32,
    audio: Option<String>,
) -> Result<(Vec<u8>, u32, u32), CmdError> {
    let backend = XcapBackend;
    backend.permission_preflight().map_err(|e| CmdError::Any(e.to_string()))?;
    let monitors = backend.list_monitors().map_err(|e| CmdError::Any(e.to_string()))?;
    let monitor = asterism_capture::preferred_monitor(&monitors)
        .ok_or_else(|| CmdError::Any("no monitor".into()))?;
    let first = backend.capture_display(monitor).map_err(|e| CmdError::Any(e.to_string()))?;
    let sel = crate::overlay_cli::select_region_subprocess(&first)
        .map_err(|e| CmdError::Any(e.to_string()))?;
    let session = OverlaySession { frame: first, selection: sel };
    let (w, h, _) = session.crop_bgra().ok_or_else(|| CmdError::Any("need selection".into()))?;
    let backend_fps = if cfg!(target_os = "macos") { fps } else { fps.min(30) };
    let plan =
        RecordingPlan::new(seconds, backend_fps).map_err(|e| CmdError::Any(e.to_string()))?;
    let fps = plan.fps;
    let audio = match audio.as_deref() {
        Some("mic") => AudioSource::Microphone,
        Some("system") => AudioSource::System,
        Some("both") => AudioSource::Both,
        _ => AudioSource::None,
    };

    #[cfg(target_os = "macos")]
    {
        let mut rec =
            MacOsRecording::start(w, h, fps, audio).map_err(|e| CmdError::Any(e.to_string()))?;
        let started = std::time::Instant::now();
        for i in 0..plan.frames {
            sleep_until(plan.deadline(started, i));
            let frame =
                backend.capture_display(monitor).map_err(|e| CmdError::Any(e.to_string()))?;
            let sess = OverlaySession { frame, selection: session.selection.clone() };
            if let Some((cw, ch, bgra)) = sess.crop_bgra() {
                let vf =
                    VideoFrame { timestamp_us: plan.timestamp_us(i), width: cw, height: ch, bgra };
                rec.push(&vf).map_err(|e| CmdError::Any(e.to_string()))?;
            }
        }
        let bytes = rec.finish().map_err(|e| CmdError::Any(e.to_string()))?;
        Ok((bytes, w, h))
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = audio;
        let mut avi = AviMjpeg::new(w, h, fps);
        let started = std::time::Instant::now();
        for i in 0..plan.frames {
            sleep_until(plan.deadline(started, i));
            let frame =
                backend.capture_display(monitor).map_err(|e| CmdError::Any(e.to_string()))?;
            let sess = OverlaySession { frame, selection: session.selection.clone() };
            if let Some((cw, ch, bgra)) = sess.crop_bgra() {
                let vf =
                    VideoFrame { timestamp_us: plan.timestamp_us(i), width: cw, height: ch, bgra };
                let _ = avi.push(&vf);
            }
        }
        let bytes = avi.finish().map_err(|e| CmdError::Any(e.to_string()))?;
        Ok((bytes, w, h))
    }
}

fn sleep_until(deadline: std::time::Instant) {
    let remaining = deadline.saturating_duration_since(std::time::Instant::now());
    if !remaining.is_zero() {
        std::thread::sleep(remaining);
    }
}

#[tauri::command]
pub async fn scroll_capture(
    state: State<'_, DesktopState>,
    frames: u32,
) -> Result<String, CmdError> {
    let (png, w, h) = tauri::async_runtime::spawn_blocking(move || scroll_capture_inner(frames))
        .await
        .map_err(|e| CmdError::Any(e.to_string()))??;
    crate::commands::insert_screenshot(&state, png, w, h)
}

fn scroll_capture_inner(frames: u32) -> Result<(Vec<u8>, u32, u32), CmdError> {
    let backend = XcapBackend;
    let monitors = backend.list_monitors().map_err(|e| CmdError::Any(e.to_string()))?;
    let monitor = asterism_capture::preferred_monitor(&monitors)
        .ok_or_else(|| CmdError::Any("no monitor".into()))?;
    let first = backend.capture_display(monitor).map_err(|e| CmdError::Any(e.to_string()))?;
    let sel = crate::overlay_cli::select_region_subprocess(&first)
        .map_err(|e| CmdError::Any(e.to_string()))?;
    let mut engine = asterism_capture::ScrollCaptureEngine::default();
    let n = frames.clamp(2, 40);
    for i in 0..n {
        if i > 0 {
            asterism_capture::ScrollCaptureEngine::inject_scroll(-80);
            std::thread::sleep(std::time::Duration::from_millis(120));
        }
        let frame = backend.capture_display(monitor).map_err(|e| CmdError::Any(e.to_string()))?;
        let sess = OverlaySession { frame: frame.clone(), selection: sel.clone() };
        if let Some((w, h, bgra)) = sess.crop_bgra() {
            let cropped = asterism_capture::CapturedFrame {
                width: w,
                height: h,
                bgra,
                monitor: frame.monitor,
            };
            let confidence = engine.push(&cropped).map_err(|e| CmdError::Any(e.to_string()))?;
            if engine.should_stop_auto() {
                tracing::warn!(confidence, "scroll capture stopped after low-confidence matches");
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(280));
    }
    let tile = engine.flatten().ok_or_else(|| CmdError::Any("no frames".into()))?;
    let png = export_png(tile.width, tile.height, &tile.bgra, &AnnotationScene::default())
        .map_err(CmdError::Any)?;
    Ok((png, tile.width, tile.height))
}

fn insert_blob(
    state: &DesktopState,
    bytes: Vec<u8>,
    w: u32,
    h: u32,
    kind: ContentKind,
) -> Result<String, CmdError> {
    let blob = state.store.put_blob(&bytes).map_err(|e| CmdError::Any(e.to_string()))?;
    let mut item = asterism_clipboard::NormalizedContent::Image {
        png: Vec::new(),
        width: w,
        height: h,
        dedup_tag: state.vault.read().avk.dedup_tag(&asterism_crypto::blake3_bytes(&bytes)),
        flags: asterism_core::ContentFlags::REMOTE_ALLOWED,
        source_app: Some("asterism".into()),
    }
    .into_item(state.identity.device_id, asterism_platform::now_ms());
    item.kind = kind;
    item.payload_ref = asterism_core::PayloadRef::Blob { blob_id: blob };
    item.logical_size = bytes.len() as u64;
    item.payload_size = bytes.len() as u64;
    let id = state.store.insert(item.clone(), None).map_err(|e| CmdError::Any(e.to_string()))?;
    state.sync.notify_local(item);
    Ok(id.to_string())
}
