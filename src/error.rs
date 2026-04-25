use thiserror::Error;

#[derive(Debug, Error)]
pub enum WdoError {
    #[error("no backend available for this environment")]
    NoBackend,

    #[error("backend '{backend}' does not support: {what}")]
    NotSupported {
        backend: &'static str,
        what: &'static str,
    },

    #[error("backend '{backend}' failed: {source}")]
    Backend {
        backend: &'static str,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[error("failed to parse key chain '{input}': {reason}")]
    Keysym { input: String, reason: String },

    #[error("window not found: {0}")]
    WindowNotFound(String),

    #[error("invalid argument: {0}")]
    InvalidArg(String),
}

pub type Result<T, E = WdoError> = std::result::Result<T, E>;
