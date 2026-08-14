use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::error::{KernelError, Result};

static NEXT_SCOPE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ScopeId(u64);

impl ScopeId {
    fn next() -> Self {
        Self(NEXT_SCOPE.fetch_add(1, Ordering::Relaxed))
    }

    pub fn raw(self) -> u64 {
        self.0
    }
}

/// 有独立取消时机的生命周期节点。
pub struct Scope {
    id: ScopeId,
    closed: Arc<AtomicBool>,
    parent_closed: Option<Arc<AtomicBool>>,
    children: Vec<Scope>,
}

impl Scope {
    pub fn root() -> Self {
        Self {
            id: ScopeId::next(),
            closed: Arc::new(AtomicBool::new(false)),
            parent_closed: None,
            children: Vec::new(),
        }
    }

    pub fn id(&self) -> ScopeId {
        self.id
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
            || self.parent_closed.as_ref().is_some_and(|flag| flag.load(Ordering::Acquire))
    }

    pub fn child(&mut self) -> Result<&mut Scope> {
        if self.is_closed() {
            return Err(KernelError::ScopeClosed);
        }
        self.children.push(self.spawn_child());
        Ok(self.children.last_mut().expect("just pushed"))
    }

    pub fn cancel_token(&self) -> CancelToken {
        CancelToken { closed: Arc::clone(&self.closed), parent_closed: self.parent_closed.clone() }
    }

    /// 独立 closed 标志；父 dispose 会取消，子 dispose 不会关闭父。
    pub fn fork(&self) -> Self {
        self.spawn_child()
    }

    fn spawn_child(&self) -> Self {
        Self {
            id: ScopeId::next(),
            closed: Arc::new(AtomicBool::new(false)),
            parent_closed: Some(Arc::clone(&self.closed)),
            children: Vec::new(),
        }
    }

    /// 先关子 Scope，再标记自身关闭。
    pub fn dispose(&mut self) {
        for child in &mut self.children {
            child.dispose();
        }
        self.closed.store(true, Ordering::Release);
    }
}

#[derive(Clone, Debug)]
pub struct CancelToken {
    closed: Arc<AtomicBool>,
    parent_closed: Option<Arc<AtomicBool>>,
}

impl CancelToken {
    pub fn is_cancelled(&self) -> bool {
        self.closed.load(Ordering::Acquire)
            || self.parent_closed.as_ref().is_some_and(|flag| flag.load(Ordering::Acquire))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disposing_parent_closes_children() {
        let mut root = Scope::root();
        let child_token = {
            let child = root.child().unwrap();
            child.cancel_token()
        };
        assert!(!child_token.is_cancelled());
        root.dispose();
        assert!(root.is_closed());
        assert!(child_token.is_cancelled());
        assert!(root.child().is_err());
    }

    #[test]
    fn fork_parent_cancel_does_not_require_child_dispose() {
        let mut root = Scope::root();
        let forked = root.fork();
        let token = forked.cancel_token();
        assert!(!token.is_cancelled());
        root.dispose();
        assert!(token.is_cancelled());
        assert!(forked.is_closed());
    }

    #[test]
    fn disposing_fork_does_not_close_parent() {
        let root = Scope::root();
        let mut forked = root.fork();
        forked.dispose();
        assert!(forked.is_closed());
        assert!(!root.is_closed());
        assert!(!root.cancel_token().is_cancelled());
    }
}
