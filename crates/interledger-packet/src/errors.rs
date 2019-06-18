use std::str::Utf8Error;
use std::string::FromUtf8Error;

use quick_error::quick_error;

use super::AddressError;

quick_error! {
    #[derive(Debug)]
    pub enum ParseError {
        Io(err: std::io::Error) {
            from()
            description(err.description())
            cause(err)
        }
        Utf8(err: Utf8Error) {
            from()
            description(err.description())
            cause(err)
        }
        FromUtf8(err: FromUtf8Error) {
            from()
            description(err.description())
            cause(err)
        }
        Chrono(err: chrono::ParseError) {
            from()
            description(err.description())
            cause(err)
        }
        WrongType(descr: String) {
            description(descr)
            display("Wrong Type {}", descr)
        }
        InvalidAddress(err: AddressError) {
            from()
            description(err.description())
            cause(err)
        }
        InvalidPacket(descr: String) {
            description(descr)
            display("Invalid Packet {}", descr)
        }
        Other(err: Box<dyn std::error::Error>) {
            cause(&**err)
            description(err.description())
            display("Error {}", err.description())
        }
    }
}
