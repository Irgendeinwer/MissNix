use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("I/O error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Network request failed ({provider}): {message}")]
    Network {
        provider: &'static str,
        message: String,
    },

    #[error("Provider rate limit exceeded ({provider}); please retry shortly")]
    RateLimited { provider: &'static str },

    #[error("Invalid response from {provider}: {message}")]
    InvalidResponse {
        provider: &'static str,
        message: String,
    },

    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Operation cancelled")]
    Cancelled,

    #[error("{0}")]
    Validation(String),
}

pub type CoreResult<T> = Result<T, CoreError>;
