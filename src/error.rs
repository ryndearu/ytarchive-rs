use thiserror::Error;

/// Crate-wide error type.
#[derive(Debug, Error)]
pub enum Error {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("http status {status} for {what}")]
    HttpStatus { status: u16, what: String },

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("yt-dlp failed: {0}")]
    Ytdlp(String),

    #[error("no download urls found for {0}")]
    NoUrls(String),

    #[error("invalid filename format: {0}")]
    InvalidFormat(String),

    #[error("invalid argument: {0}")]
    InvalidArg(String),

    #[error("cancelled")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, Error>;
