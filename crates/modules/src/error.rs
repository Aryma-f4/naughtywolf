/// Errors that can occur during module execution or registry operations.
#[derive(Debug, thiserror::Error)]
pub enum ModuleError {
    #[error("module not found: {0}")]
    NotFound(String),

    #[error("invalid arguments for {0}: {1}")]
    BadArgs(String, String),

    #[error("module execution failed: {0}")]
    Execution(String),

    #[error("output serialization failed: {0}")]
    Serialization(String),
}
