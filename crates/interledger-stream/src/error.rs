use failure::Fail;

#[derive(Fail, Debug)]
pub enum Error {
    #[fail(display = "Error connecting: {}", _0)]
    ConnectionError(String),
    #[fail(display = "Error polling: {}", _0)]
    PollError(String),
    #[fail(display = "Error polling: {}", _0)]
    SendMoneyError(String),
    #[fail(display = "Error maximum time exceeded: {}", _0)]
    TimeoutError(String),
}
