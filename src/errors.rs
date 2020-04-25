use std::num::ParseIntError;
use std::str::Utf8Error;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("ProtocolError")]
    ProtocolError,
    #[error("ClientClosed")]
    ClientClosed,
    #[error("SendBufferFull")]
    SendBufferFull,
    #[error("IOError: {0}")]
    IOError(#[from] std::io::Error),
    #[error("EncodingError: {0}")]
    EncodingError(#[from] Utf8Error),
    #[error("ConnectionTimeout")]
    ConnectionTimeout,
    #[error("SerdeError: {0}")]
    SerdeError(#[from] serde_json::error::Error),
    #[error("ParseIntError: {0}")]
    ParseIntError(#[from] ParseIntError),
}

/*
impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Error::IOError(err)
    }
}

impl From<Utf8Error> for Error {
    fn from(err: Utf8Error) -> Self {
        Error::EncodingError(err)
    }
}

impl From<ParseIntError> for Error {
    fn from(_: ParseIntError) -> Self {
        Error::ProtocolError
    }
}

impl From<serde_json::error::Error> for Error {
    fn from(_: serde_json::error::Error) -> Error {
        Error::ProtocolError
    }
}
*/
