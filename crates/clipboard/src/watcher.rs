use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use asterism_core::id::DeviceId;
use asterism_core::policy::CapturePolicy;

use crate::capture::{ClipboardBackend, NativeClipboard};
use crate::error::Result;
use crate::guard::SelfWriteGuard;
use crate::normalize::{self, NormalizedContent};

#[derive(Clone, Debug)]
pub struct WatcherConfig {
    pub poll_interval: Duration,
    pub policy: CapturePolicy,
    pub device_id: DeviceId,
}

impl Default for WatcherConfig {
    fn default() -> Self {
        Self {
            // macOS changeCount 低频检查；Windows 实现仍走事件，此间隔仅作兜底。
            poll_interval: Duration::from_millis(350),
            policy: CapturePolicy::default(),
            device_id: DeviceId::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub enum ClipboardEvent {
    Captured(NormalizedContent),
    Ignored,
    Error(String),
}

pub fn spawn_watcher(
    config: WatcherConfig,
    guard: Arc<SelfWriteGuard>,
    on_event: impl Fn(ClipboardEvent) + Send + 'static,
) -> WatcherHandle {
    spawn_with_backend(config, guard, NativeClipboard, on_event)
}

pub fn spawn_with_backend<B>(
    config: WatcherConfig,
    guard: Arc<SelfWriteGuard>,
    backend: B,
    on_event: impl Fn(ClipboardEvent) + Send + 'static,
) -> WatcherHandle
where
    B: ClipboardBackend + 'static,
{
    let running = Arc::new(AtomicBool::new(true));
    let flag = Arc::clone(&running);
    thread::Builder::new()
        .name("asterism-clipboard".into())
        .spawn(move || {
            if let Err(err) = run_loop(&config, &guard, &backend, &flag, &on_event) {
                on_event(ClipboardEvent::Error(err.to_string()));
            }
        })
        .expect("spawn clipboard watcher");
    WatcherHandle { running }
}

fn run_loop<B: ClipboardBackend>(
    config: &WatcherConfig,
    guard: &SelfWriteGuard,
    backend: &B,
    running: &AtomicBool,
    on_event: &impl Fn(ClipboardEvent),
) -> Result<()> {
    #[cfg(windows)]
    let win_signal = crate::windows::spawn_update_signal();
    let mut last = backend.change_token().unwrap_or(0);
    while running.load(Ordering::Relaxed) {
        #[cfg(windows)]
        {
            // 主路径：WM_CLIPBOARDUPDATE。超时仅用于线程退出与漏事件兜底。
            let _ = win_signal.recv_timeout(config.poll_interval);
        }
        #[cfg(not(windows))]
        thread::sleep(config.poll_interval);
        let token = match backend.change_token() {
            Ok(t) => t,
            Err(err) => {
                on_event(ClipboardEvent::Error(err.to_string()));
                continue;
            }
        };
        if token == last {
            continue;
        }
        last = token;
        match backend.read() {
            Ok(Some(captured)) => match normalize::normalize(&captured, &config.policy) {
                Ok(Some(content)) => {
                    if guard.is_self_write(None, &content.dedup_tag()) {
                        on_event(ClipboardEvent::Ignored);
                        continue;
                    }
                    on_event(ClipboardEvent::Captured(content));
                }
                Ok(None) => on_event(ClipboardEvent::Ignored),
                Err(err) => on_event(ClipboardEvent::Error(err.to_string())),
            },
            Ok(None) => on_event(ClipboardEvent::Ignored),
            Err(err) => on_event(ClipboardEvent::Error(err.to_string())),
        }
    }
    Ok(())
}

pub struct WatcherHandle {
    running: Arc<AtomicBool>,
}

impl WatcherHandle {
    pub fn stop(&self) {
        self.running.store(false, Ordering::Relaxed);
    }
}

impl Drop for WatcherHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::CapturedClipboard;
    use crate::error::Result as ClipResult;
    use parking_lot::Mutex;
    use std::sync::mpsc;

    struct Fake {
        tokens: Mutex<Vec<u64>>,
        payload: CapturedClipboard,
    }

    impl ClipboardBackend for Fake {
        fn change_token(&self) -> ClipResult<u64> {
            let mut tokens = self.tokens.lock();
            if tokens.len() > 1 {
                return Ok(tokens.remove(0));
            }
            Ok(tokens[0])
        }

        fn read(&self) -> ClipResult<Option<CapturedClipboard>> {
            Ok(Some(self.payload.clone()))
        }

        fn write(&self, _: &NormalizedContent) -> ClipResult<()> {
            Ok(())
        }
    }

    #[test]
    fn emits_on_change_and_swallows_self_write() {
        let payload = CapturedClipboard {
            change_token: 2,
            source_app: None,
            formats: vec!["public.utf8-plain-text".into()],
            text: Some("hello-watcher".into()),
            image: None,
            files: vec![],
            sensitive: false,
        };
        let tag = asterism_crypto::local_dedup_tag(b"hello-watcher");
        let guard = Arc::new(SelfWriteGuard::default());
        guard.remember(asterism_core::id::ContentId::new(), tag);

        let (tx, rx) = mpsc::channel();
        let handle = spawn_with_backend(
            WatcherConfig { poll_interval: Duration::from_millis(20), ..WatcherConfig::default() },
            Arc::clone(&guard),
            Fake { tokens: Mutex::new(vec![1, 2]), payload },
            move |ev| {
                let _ = tx.send(ev);
            },
        );
        let ev = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        handle.stop();
        assert!(matches!(ev, ClipboardEvent::Ignored));
    }
}
