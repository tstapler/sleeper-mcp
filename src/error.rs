use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("API error {status}: {message}")]
    Api { status: u16, message: String },

    #[error("Rate limited after {retries} retries")]
    RateLimited { retries: u32 },

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("Task join error: {0}")]
    Join(#[from] tokio::task::JoinError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(#[from] anyhow::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
