use chrono;
use std;
use std::str::Utf8Error;
use std::string::FromUtf8Error;

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("I/O Error: {0}")]
    IoErr(#[from] std::io::Error),
    #[error("UTF-8 Error: {0}")]
    Utf8Err(#[from] Utf8Error),
    #[error("UTF-8 Conversion Error: {0}")]
    FromUtf8Err(#[from] FromUtf8Error),
    #[error("Chrono Error: {0}")]
    ChronoErr(#[from] chrono::ParseError),
    #[error("Invalid Packet: {0}")]
    InvalidPacket(String),
}
