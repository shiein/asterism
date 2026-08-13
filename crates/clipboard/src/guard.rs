use std::collections::HashMap;
use std::time::{Duration, Instant};

use asterism_core::id::ContentId;
use parking_lot::Mutex;

/// 远端/本机回写剪贴板前登记，Watcher 再次收到系统变化时吞掉自写事件。
pub struct SelfWriteGuard {
    inner: Mutex<Inner>,
    window: Duration,
}

struct Inner {
    by_id: HashMap<ContentId, Instant>,
    by_tag: HashMap<[u8; 32], Instant>,
}

impl SelfWriteGuard {
    pub fn new(window: Duration) -> Self {
        Self { inner: Mutex::new(Inner { by_id: HashMap::new(), by_tag: HashMap::new() }), window }
    }

    pub fn remember(&self, id: ContentId, dedup_tag: [u8; 32]) {
        let mut inner = self.inner.lock();
        let now = Instant::now();
        inner.gc(now, self.window);
        inner.by_id.insert(id, now);
        inner.by_tag.insert(dedup_tag, now);
    }

    pub fn is_self_write(&self, id: Option<ContentId>, dedup_tag: &[u8; 32]) -> bool {
        let mut inner = self.inner.lock();
        let now = Instant::now();
        inner.gc(now, self.window);
        if let Some(id) = id {
            if inner.by_id.contains_key(&id) {
                return true;
            }
        }
        inner.by_tag.contains_key(dedup_tag)
    }
}

impl Inner {
    fn gc(&mut self, now: Instant, window: Duration) {
        self.by_id.retain(|_, t| now.duration_since(*t) < window);
        self.by_tag.retain(|_, t| now.duration_since(*t) < window);
    }
}

impl Default for SelfWriteGuard {
    fn default() -> Self {
        Self::new(Duration::from_millis(1500))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swallows_within_window() {
        let guard = SelfWriteGuard::new(Duration::from_millis(200));
        let id = ContentId::new();
        let tag = [3u8; 32];
        guard.remember(id, tag);
        assert!(guard.is_self_write(Some(id), &tag));
        assert!(guard.is_self_write(None, &tag));
        std::thread::sleep(Duration::from_millis(250));
        assert!(!guard.is_self_write(Some(id), &tag));
    }
}
