use thiserror::Error;

pub type Result<T> = std::result::Result<T, Ps2Error>;

#[derive(Debug, Error)]
pub enum Ps2Error {
    #[error("erro de I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("imagem ISO inválida: {0}")]
    InvalidIso(String),
    #[error("arquivo ZSO inválido: {0}")]
    InvalidZso(String),
    #[error("falha HTTP: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Game ID não encontrado: {0}")]
    GameIdNotFound(String),
    #[error("VMC inválida: {0}")]
    InvalidVmc(String),
    #[error("PSU inválido: {0}")]
    InvalidPsu(String),
    #[error("{0}")]
    Other(String),
}
