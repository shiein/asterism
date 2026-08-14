use thiserror::Error;

pub type Result<T> = std::result::Result<T, KernelError>;

#[derive(Debug, Error)]
pub enum KernelError {
    #[error("service already registered")]
    ServiceConflict,
    #[error("required service is missing")]
    ServiceMissing,
    #[error("permission denied: {0}")]
    PermissionDenied(&'static str),
    #[error("scope is closed")]
    ScopeClosed,
    #[error("mount failed: {0}")]
    Mount(String),
    #[error("invalid plugin id: {0}")]
    InvalidId(String),
    #[error("duplicate plugin: {0}")]
    DuplicatePlugin(String),
    #[error("missing plugin dependency: {0} requires {1}")]
    MissingDependency(String, String),
    #[error("plugin dependency cycle: {0}")]
    DependencyCycle(String),
}
