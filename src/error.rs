use thiserror::Error;

/// Crate-wide error type. Fragment-level errors carry enough context for the
/// retry/refresh policy to decide what to do next.
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

    #[error("video details not found; video is likely private or does not exist")]
    VideoUnavailable,

    #[error("no download urls found for {0}")]
    NoUrls(String),

    #[error("invalid quality selection: {0}")]
    InvalidQuality(String),

    #[error("invalid filename format: {0}")]
    InvalidFormat(String),

    #[error("ump parse error: {0}")]
    Ump(String),

    #[error("ffmpeg exited with status {0}")]
    Ffmpeg(i32),

    #[error("cancelled")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, Error>;
