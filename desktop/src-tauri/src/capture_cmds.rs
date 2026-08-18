use asterism_capture::backend::CaptureBackend;
use asterism_capture::{
    AnnotationScene, MonitorInfo, OverlaySession, Selection, XcapBackend, export_png,
};
use asterism_core::ContentKind;
use asterism_media::AudioSource;
use asterism_media::VideoFrame;
#[cfg(not(target_os = "macos"))]
use asterism_media::avi::AviMjpeg;
use asterism_media::gifenc::GifSession;
#[cfg(target_os = "macos")]
use asterism_media::macos::MacOsRecording;
use base64::Engine;
use tauri::{AppHandle, State};

use crate::commands::{CmdError, insert_screenshot};
use crate::runtime::{DesktopState, RecordingLease};

#[derive(serde::Serialize)]
pub struct AnnotationSource {
    data_url: String,
    width: u32,
    height: u32,
}

#[tauri::command]
pub fn preview_image(state: State<'_, DesktopState>, item_id: String) -> Result<String, CmdError> {
    let (_, bytes, _, _) = load_image_item(&state, &item_id)?;
    encode_preview_data_url(&bytes)
}

fn encode_preview_data_url(bytes: &[u8]) -> Result<String, CmdError> {
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Ok(format!(
            "data:image/gif;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        ));
    }
    const MAX_EDGE: u32 = 480;
    let img = image::load_from_memory(bytes).map_err(|e| CmdError::Any(e.to_string()))?;
    let img = if img.width() > MAX_EDGE || img.height() > MAX_EDGE {
        img.thumbnail(MAX_EDGE, MAX_EDGE)
    } else {
        img
    };
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| CmdError::Any(e.to_string()))?;
    Ok(format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png)))
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
    XcapBackend.permission_preflight().map_err(|e| CmdError::Any(e.to_string()))?;
    XcapBackend.list_windows().map_err(|e| CmdError::Any(e.to_string()))
}

#[tauri::command]
pub async fn capture_window(
    app: AppHandle,
    state: State<'_, DesktopState>,
    id: u32,
) -> Result<String, CmdError> {
    let session = state.begin_capture();
    if session.is_cancelled() {
        return Err(CmdError::Any("cancelled".into()));
    }
    crate::commands::ensure_capture_permission().await?;
    let hidden = crate::capture_ui::HiddenMainWindow::hide(&app)?;
    hidden.wait_until_not_captured().await;
    let frame = tauri::async_runtime::spawn_blocking(move || XcapBackend.capture_window(id))
        .await
        .map_err(|err| CmdError::Any(err.to_string()))?
        .map_err(|err| CmdError::Any(err.to_string()))?;
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

pub(crate) fn load_image_item(
    state: &DesktopState,
    item_id: &str,
) -> Result<(asterism_core::ContentId, Vec<u8>, u32, u32), CmdError> {
    let id = item_id.parse().map_err(|e: asterism_core::CoreError| CmdError::Any(e.to_string()))?;
    let lookup =
        state.broker.grant_read(id).ok_or_else(|| CmdError::Any("read grant denied".into()))?;
    let item = state.query().get(&lookup, id).map_err(|e| CmdError::Any(e.to_string()))?;
    let grant =
        state.broker.grant_read(id).ok_or_else(|| CmdError::Any("read grant denied".into()))?;
    let png =
        state.query().payload_bytes(&grant, &item).map_err(|e| CmdError::Any(e.to_string()))?;
    let img = image::load_from_memory(&png).map_err(|e| CmdError::Any(e.to_string()))?;
    Ok((id, png, img.width(), img.height()))
}

#[tauri::command]
pub async fn record_gif(
    app: AppHandle,
    state: State<'_, DesktopState>,
    fps: u16,
) -> Result<String, CmdError> {
    let session = state.begin_capture();
    let token = session.cancel_token();
    crate::commands::ensure_capture_permission().await?;
    let recording = state.recording.begin().map_err(CmdError::from)?;
    let overlay = crate::overlay_cli::spawn_overlay().map_err(CmdError::from)?;
    let main_window = crate::capture_ui::HiddenMainWindow::hide(&app)?;
    main_window.wait_until_not_captured().await;
    let target = tauri::async_runtime::spawn_blocking({
        let token = token.clone();
        move || select_recording_target(token, overlay)
    })
    .await
    .map_err(|err| CmdError::Any(err.to_string()))??;
    let (toolbar, starts_at) = crate::capture_ui::RecordingToolbar::show(
        &app,
        &target.monitor,
        &target.selection,
        "gif",
        std::time::Duration::from_secs(3),
    )?;
    let (bytes, w, h) = tauri::async_runtime::spawn_blocking(move || {
        record_gif_inner(target, fps, token, recording, starts_at)
    })
    .await
    .map_err(|e| CmdError::Any(e.to_string()))??;
    drop(toolbar);
    drop(main_window);
    insert_blob(&state, bytes, w, h, ContentKind::Gif, Some("image/gif"))
}

fn record_gif_inner(
    target: RecordingTarget,
    fps: u16,
    token: asterism_kernel::CancelToken,
    recording: RecordingLease,
    starts_at: std::time::Instant,
) -> Result<(Vec<u8>, u32, u32), CmdError> {
    let backend = XcapBackend;
    wait_for_recording_start(starts_at, &token, &recording)?;
    let fps = fps.clamp(8, 15);
    let mut gif = GifSession::new(target.width, target.height, fps)
        .map_err(|e| CmdError::Any(e.to_string()))?;
    let started = std::time::Instant::now();
    let mut frame_index = 0_u64;
    loop {
        if frame_index > 0 && recording.stop_requested() {
            break;
        }
        ensure_recording_alive(&token)?;
        let deadline = recording_deadline(started, frame_index, u32::from(fps));
        wait_until_recording_deadline(deadline, &token)?;
        let frame =
            backend.capture_display(&target.monitor).map_err(|e| CmdError::Any(e.to_string()))?;
        let sess = OverlaySession { frame, selection: Some(target.selection.clone()) };
        if let Some((cw, ch, bgra)) = sess.crop_bgra() {
            let vf = VideoFrame {
                timestamp_us: recording_timestamp_us(frame_index, u32::from(fps)),
                width: cw,
                height: ch,
                bgra,
            };
            gif.push(&vf).map_err(|e| CmdError::Any(e.to_string()))?;
        }
        frame_index = frame_index.saturating_add(1);
    }
    let bytes = gif.finish().map_err(|e| CmdError::Any(e.to_string()))?;
    Ok((bytes, target.width, target.height))
}

#[tauri::command]
pub async fn record_video(
    app: AppHandle,
    state: State<'_, DesktopState>,
    fps: u32,
    audio: Option<String>,
) -> Result<String, CmdError> {
    let session = state.begin_capture();
    let token = session.cancel_token();
    crate::commands::ensure_capture_permission().await?;
    #[cfg(target_os = "macos")]
    if audio.as_deref().is_some_and(|source| matches!(source, "mic" | "both"))
        && !asterism_media::macos::mic_access_ok()
    {
        asterism_media::macos::request_mic_access();
        return Err(CmdError::Any(
            "microphone permission requested; grant it, then start recording again".into(),
        ));
    }
    let recording = state.recording.begin().map_err(CmdError::from)?;
    let overlay = crate::overlay_cli::spawn_overlay().map_err(CmdError::from)?;
    let main_window = crate::capture_ui::HiddenMainWindow::hide(&app)?;
    main_window.wait_until_not_captured().await;
    let target = tauri::async_runtime::spawn_blocking({
        let token = token.clone();
        move || select_recording_target(token, overlay)
    })
    .await
    .map_err(|err| CmdError::Any(err.to_string()))??;
    let (toolbar, starts_at) = crate::capture_ui::RecordingToolbar::show(
        &app,
        &target.monitor,
        &target.selection,
        "video",
        std::time::Duration::from_secs(3),
    )?;
    let (bytes, w, h, mime) = tauri::async_runtime::spawn_blocking(move || {
        record_video_inner(target, fps, audio, token, recording, starts_at)
    })
    .await
    .map_err(|e| CmdError::Any(e.to_string()))??;
    drop(toolbar);
    drop(main_window);
    insert_blob(&state, bytes, w, h, ContentKind::Video, Some(mime))
}

fn record_video_inner(
    target: RecordingTarget,
    fps: u32,
    audio: Option<String>,
    token: asterism_kernel::CancelToken,
    recording: RecordingLease,
    starts_at: std::time::Instant,
) -> Result<(Vec<u8>, u32, u32, &'static str), CmdError> {
    #[cfg(not(target_os = "macos"))]
    if audio.as_deref().is_some_and(|source| source != "none") {
        tracing::info!("当前平台录制暂未包含音频轨道，使用纯画面录制");
    }
    wait_for_recording_start(starts_at, &token, &recording)?;
    let backend = XcapBackend;
    let fps = if cfg!(target_os = "macos") { fps.clamp(10, 60) } else { fps.clamp(10, 30) };
    let audio = match audio.as_deref() {
        Some("mic") => AudioSource::Microphone,
        Some("system") => AudioSource::System,
        Some("both") => AudioSource::Both,
        _ => AudioSource::None,
    };

    #[cfg(target_os = "macos")]
    {
        let mut rec = MacOsRecording::start(target.width, target.height, fps, audio)
            .map_err(|e| CmdError::Any(e.to_string()))?;
        let started = std::time::Instant::now();
        let mut frame_index = 0_u64;
        loop {
            if frame_index > 0 && recording.stop_requested() {
                break;
            }
            ensure_recording_alive(&token)?;
            wait_until_recording_deadline(recording_deadline(started, frame_index, fps), &token)?;
            let frame = backend
                .capture_display(&target.monitor)
                .map_err(|e| CmdError::Any(e.to_string()))?;
            let sess = OverlaySession { frame, selection: Some(target.selection.clone()) };
            if let Some((cw, ch, bgra)) = sess.crop_bgra() {
                let vf = VideoFrame {
                    timestamp_us: recording_timestamp_us(frame_index, fps),
                    width: cw,
                    height: ch,
                    bgra,
                };
                rec.push(&vf).map_err(|e| CmdError::Any(e.to_string()))?;
            }
            frame_index = frame_index.saturating_add(1);
        }
        let bytes = rec.finish().map_err(|e| CmdError::Any(e.to_string()))?;
        Ok((bytes, target.width, target.height, "video/mp4"))
    }

    #[cfg(windows)]
    {
        let _ = audio;
        let mp4_path =
            std::env::temp_dir().join(format!("asterism_rec_{}.mp4", uuid::Uuid::now_v7()));
        let encoder_res = asterism_media::wmf::WmfH264Encoder::create(
            &mp4_path,
            target.width,
            target.height,
            fps,
            None,
        );

        match encoder_res {
            Ok(mut encoder) => {
                let started = std::time::Instant::now();
                let mut frame_index = 0_u64;
                loop {
                    if frame_index > 0 && recording.stop_requested() {
                        break;
                    }
                    ensure_recording_alive(&token)?;
                    wait_until_recording_deadline(
                        recording_deadline(started, frame_index, fps),
                        &token,
                    )?;
                    let frame = backend
                        .capture_display(&target.monitor)
                        .map_err(|e| CmdError::Any(e.to_string()))?;
                    let sess = OverlaySession { frame, selection: Some(target.selection.clone()) };
                    if let Some((_, _, bgra)) = sess.crop_bgra() {
                        encoder.write_frame(&bgra).map_err(|e| CmdError::Any(e.to_string()))?;
                    }
                    frame_index = frame_index.saturating_add(1);
                }
                encoder.finish().map_err(|e| CmdError::Any(e.to_string()))?;
                let bytes = std::fs::read(&mp4_path).map_err(|e| CmdError::Any(e.to_string()))?;
                let _ = std::fs::remove_file(&mp4_path);
                Ok((bytes, target.width, target.height, "video/mp4"))
            }
            Err(wmf_err) => {
                tracing::warn!(error = %wmf_err, "WMF 硬件编码器不可用，自动降级为通用 MJPEG 视频编码");
                let mut avi = AviMjpeg::new(target.width, target.height, fps);
                let started = std::time::Instant::now();
                let mut frame_index = 0_u64;
                loop {
                    if frame_index > 0 && recording.stop_requested() {
                        break;
                    }
                    ensure_recording_alive(&token)?;
                    wait_until_recording_deadline(
                        recording_deadline(started, frame_index, fps),
                        &token,
                    )?;
                    let frame = backend
                        .capture_display(&target.monitor)
                        .map_err(|e| CmdError::Any(e.to_string()))?;
                    let sess = OverlaySession { frame, selection: Some(target.selection.clone()) };
                    if let Some((cw, ch, bgra)) = sess.crop_bgra() {
                        let vf = VideoFrame {
                            timestamp_us: recording_timestamp_us(frame_index, fps),
                            width: cw,
                            height: ch,
                            bgra,
                        };
                        avi.push(&vf).map_err(|e| CmdError::Any(e.to_string()))?;
                    }
                    frame_index = frame_index.saturating_add(1);
                }
                let bytes = avi.finish().map_err(|e| CmdError::Any(e.to_string()))?;
                Ok((bytes, target.width, target.height, "video/x-msvideo"))
            }
        }
    }

    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = audio;
        let mut avi = AviMjpeg::new(target.width, target.height, fps);
        let started = std::time::Instant::now();
        let mut frame_index = 0_u64;
        loop {
            if frame_index > 0 && recording.stop_requested() {
                break;
            }
            ensure_recording_alive(&token)?;
            wait_until_recording_deadline(recording_deadline(started, frame_index, fps), &token)?;
            let frame = backend
                .capture_display(&target.monitor)
                .map_err(|e| CmdError::Any(e.to_string()))?;
            let sess = OverlaySession { frame, selection: Some(target.selection.clone()) };
            if let Some((cw, ch, bgra)) = sess.crop_bgra() {
                let vf = VideoFrame {
                    timestamp_us: recording_timestamp_us(frame_index, fps),
                    width: cw,
                    height: ch,
                    bgra,
                };
                avi.push(&vf).map_err(|e| CmdError::Any(e.to_string()))?;
            }
            frame_index = frame_index.saturating_add(1);
        }
        let bytes = avi.finish().map_err(|e| CmdError::Any(e.to_string()))?;
        Ok((bytes, target.width, target.height, "video/x-msvideo"))
    }
}

#[derive(Clone)]
struct RecordingTarget {
    monitor: MonitorInfo,
    selection: Selection,
    width: u32,
    height: u32,
}

fn select_recording_target(
    token: asterism_kernel::CancelToken,
    mut overlay: crate::overlay_cli::OverlayProcess,
) -> Result<RecordingTarget, CmdError> {
    ensure_recording_alive(&token)?;
    let backend = XcapBackend;
    let monitors = backend.list_monitors().map_err(|e| CmdError::Any(e.to_string()))?;
    let monitor = asterism_capture::preferred_monitor(&monitors)
        .cloned()
        .ok_or_else(|| CmdError::Any("no monitor".into()))?;
    let first = backend.capture_display(&monitor).map_err(|e| CmdError::Any(e.to_string()))?;
    overlay.submit(&first).map_err(|e| CmdError::Any(e.to_string()))?;
    let selection = overlay
        .wait(Some(&token))
        .map_err(|e| CmdError::Any(e.to_string()))?
        .and_then(selection_of)
        .ok_or_else(|| CmdError::Any("cancelled".into()))?;
    let session = OverlaySession { frame: first, selection: Some(selection.clone()) };
    let (width, height, _) =
        session.crop_bgra().ok_or_else(|| CmdError::Any("empty selection".into()))?;
    Ok(RecordingTarget { monitor, selection, width, height })
}

/// 录制与滚动截图只关心选区，忽略 overlay 返回的标注场景。
fn selection_of(outcome: asterism_capture::OverlayOutcome) -> Option<Selection> {
    match outcome {
        asterism_capture::OverlayOutcome::Complete { selection, .. }
        | asterism_capture::OverlayOutcome::Download { selection, .. }
        | asterism_capture::OverlayOutcome::Pin { selection, .. }
        | asterism_capture::OverlayOutcome::Scroll { selection } => Some(selection),
        asterism_capture::OverlayOutcome::Cancel => None,
    }
}

fn ensure_recording_alive(token: &asterism_kernel::CancelToken) -> Result<(), CmdError> {
    if token.is_cancelled() {
        return Err(CmdError::Any("cancelled".into()));
    }
    Ok(())
}

fn wait_for_recording_start(
    starts_at: std::time::Instant,
    token: &asterism_kernel::CancelToken,
    recording: &RecordingLease,
) -> Result<(), CmdError> {
    wait_until_recording_deadline(starts_at, token)?;
    if recording.stop_requested() {
        return Err(CmdError::Any("cancelled".into()));
    }
    Ok(())
}

fn wait_until_recording_deadline(
    deadline: std::time::Instant,
    token: &asterism_kernel::CancelToken,
) -> Result<(), CmdError> {
    loop {
        ensure_recording_alive(token)?;
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return Ok(());
        }
        std::thread::sleep(remaining.min(std::time::Duration::from_millis(20)));
    }
}

fn recording_timestamp_us(frame_index: u64, fps: u32) -> u64 {
    frame_index.saturating_mul(1_000_000) / u64::from(fps.max(1))
}

fn recording_deadline(
    started: std::time::Instant,
    frame_index: u64,
    fps: u32,
) -> std::time::Instant {
    started + std::time::Duration::from_micros(recording_timestamp_us(frame_index, fps))
}

#[tauri::command]
pub fn stop_recording(state: State<'_, DesktopState>) -> bool {
    state.recording.request_stop()
}

#[tauri::command]
pub async fn scroll_capture(
    app: AppHandle,
    state: State<'_, DesktopState>,
    frames: u32,
) -> Result<String, CmdError> {
    let session = state.begin_capture();
    let token = session.cancel_token();
    crate::commands::ensure_capture_permission().await?;
    let overlay = crate::overlay_cli::spawn_overlay().map_err(CmdError::from)?;
    let hidden = crate::capture_ui::HiddenMainWindow::hide(&app)?;
    hidden.wait_until_not_captured().await;
    let (png, w, h) =
        tauri::async_runtime::spawn_blocking(move || scroll_capture_inner(frames, token, overlay))
            .await
            .map_err(|e| CmdError::Any(e.to_string()))??;
    crate::commands::insert_screenshot(&state, png, w, h)
}

fn scroll_capture_inner(
    frames: u32,
    token: asterism_kernel::CancelToken,
    mut overlay: crate::overlay_cli::OverlayProcess,
) -> Result<(Vec<u8>, u32, u32), CmdError> {
    if token.is_cancelled() {
        return Err(CmdError::Any("cancelled".into()));
    }
    let backend = XcapBackend;
    let monitors = backend.list_monitors().map_err(|e| CmdError::Any(e.to_string()))?;
    let monitor = asterism_capture::preferred_monitor(&monitors)
        .ok_or_else(|| CmdError::Any("no monitor".into()))?;
    let first = backend.capture_display(monitor).map_err(|e| CmdError::Any(e.to_string()))?;
    overlay.submit(&first).map_err(|e| CmdError::Any(e.to_string()))?;
    let sel = overlay
        .wait(Some(&token))
        .map_err(|e| CmdError::Any(e.to_string()))?
        .and_then(selection_of);
    let mut engine = asterism_capture::ScrollCaptureEngine::default();
    let n = frames.clamp(2, 40);
    for i in 0..n {
        if token.is_cancelled() {
            return Err(CmdError::Any("cancelled".into()));
        }
        if i > 0 {
            asterism_capture::ScrollCaptureEngine::inject_scroll(-80);
            for _ in 0..12 {
                if token.is_cancelled() {
                    return Err(CmdError::Any("cancelled".into()));
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
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
    mime_hint: Option<&str>,
) -> Result<String, CmdError> {
    let id = crate::runtime::ingest_image(state, bytes, w, h, kind, mime_hint, "asterism.capture")
        .map_err(|e| CmdError::Any(e.to_string()))?;
    Ok(id.to_string())
}

#[cfg(test)]
mod tests {
    use super::encode_preview_data_url;

    #[test]
    fn preview_downscales_wide_png() {
        let img = image::RgbaImage::from_pixel(800, 200, image::Rgba([10, 20, 30, 255]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let url = encode_preview_data_url(&png).unwrap();
        let b64 = url.strip_prefix("data:image/png;base64,").expect("data url");
        let bytes =
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64).unwrap();
        let out = image::load_from_memory(&bytes).unwrap();
        assert!(out.width() <= 480);
        assert!(out.height() <= 480);
        assert!(out.width() > 0 && out.height() > 0);
    }
}
