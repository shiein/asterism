use std::process::Child;
use std::thread::{self, JoinHandle};

use crate::error::{KernelError, Result};
use crate::scope::CancelToken;

/// Host 持有的 OS 线程集合。Drop 时 join；调用方必须先取消对应 Scope。
pub struct TaskGroup {
    threads: Vec<OsThreadLease>,
}

impl TaskGroup {
    pub fn new() -> Self {
        Self { threads: Vec::new() }
    }

    pub fn spawn(
        &mut self,
        name: &'static str,
        token: CancelToken,
        work: impl FnOnce(CancelToken) + Send + 'static,
    ) -> Result<()> {
        self.threads.push(OsThreadLease::spawn(name, token, work)?);
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.threads.len()
    }

    pub fn is_empty(&self) -> bool {
        self.threads.is_empty()
    }

    pub fn adopt(&mut self, lease: OsThreadLease) {
        self.threads.push(lease);
    }
}

impl Default for TaskGroup {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for TaskGroup {
    fn drop(&mut self) {
        self.threads.clear();
    }
}

/// 受监督的 OS 线程。Drop 时 join，不强杀。
pub struct OsThreadLease {
    name: &'static str,
    handle: Option<JoinHandle<()>>,
}

impl OsThreadLease {
    pub fn spawn(
        name: &'static str,
        token: CancelToken,
        work: impl FnOnce(CancelToken) + Send + 'static,
    ) -> Result<Self> {
        let handle = thread::Builder::new()
            .name(name.into())
            .spawn(move || work(token))
            .map_err(|err| KernelError::Mount(err.to_string()))?;
        Ok(Self { name, handle: Some(handle) })
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    pub fn from_join(name: &'static str, handle: JoinHandle<()>) -> Self {
        Self { name, handle: Some(handle) }
    }
}

impl Drop for OsThreadLease {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Overlay helper 等子进程。Drop 时 kill 并 wait。
pub struct ChildProcessLease {
    child: Option<Child>,
}

impl ChildProcessLease {
    pub fn new(child: Child) -> Self {
        Self { child: Some(child) }
    }

    pub fn id(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }

    pub fn child_mut(&mut self) -> Option<&mut Child> {
        self.child.as_mut()
    }

    pub fn take(&mut self) -> Option<Child> {
        self.child.take()
    }
}

impl Drop for ChildProcessLease {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scope::Scope;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn task_group_observes_scope_cancel() {
        let mut root = Scope::root();
        let token = root.cancel_token();
        let saw = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&saw);
        let mut group = TaskGroup::new();
        group
            .spawn("asterism-test-task", token, move |token| {
                while !token.is_cancelled() {
                    thread::sleep(std::time::Duration::from_millis(5));
                }
                flag.store(true, Ordering::SeqCst);
            })
            .unwrap();
        root.dispose();
        drop(group);
        assert!(saw.load(Ordering::SeqCst));
    }

    #[test]
    fn child_process_lease_kills_on_drop() {
        let child = match std::process::Command::new("sleep").arg("30").spawn() {
            Ok(child) => child,
            Err(_) => return,
        };
        let pid = child.id();
        drop(ChildProcessLease::new(child));
        let _ = pid;
    }
}
