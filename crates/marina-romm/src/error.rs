use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("RomM request failed: {0}")]
    Request(#[from] reqwest::Error),

    #[error("RomM returned HTTP {status}: {body}")]
    Http { status: u16, body: String },

    #[error("RomM returned invalid JSON: {0}")]
    Json(#[from] serde_json::Error),

    #[error("failed to encode RomM query: {0}")]
    Query(#[from] serde_urlencoded::ser::Error),

    #[error("invalid authorization header")]
    InvalidHeader,

    #[error("failed to write downloaded RomM file: {0}")]
    Io(#[from] std::io::Error),
}
