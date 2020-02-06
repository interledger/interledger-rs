/// Stream Errors
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Error connecting: {0}")]
    ConnectionError(String),
    #[error("Error polling: {0}")]
    PollError(String),
    #[error("Error polling: {0}")]
    SendMoneyError(String),
    #[error("Error maximum time exceeded: {0}")]
    TimeoutError(String),
}
