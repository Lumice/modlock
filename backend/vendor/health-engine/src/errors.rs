#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    ModInvalid(String),
    #[error("{0}")]
    InvalidInput(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
