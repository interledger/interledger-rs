use std;
use std::error::Error;
use serde_json;
use base64;

quick_error! {
    #[derive(Debug)]
    pub enum ParseError {
        Io(err: std::io::Error) {
            from()
            description(err.description())
            cause(err)
        }
        Json(err: serde_json::Error) {
        //Json(descr: &'static str) {
            //from(err: serde_json::Error) -> (err.description())
            //from(err: serde::de::value::Error) -> (err.description())
            //from(err: serde::ser::Error) -> (err.description())
            //from(err: serde::Deserializer<'de>::Error) -> ("json parsing error", err.description())
            //description(descr)
            description(err.description())
            display("JSON Parsing Error {}", err.description())
        }
        WrongType(descr: &'static str) {
            description(descr)
            display("Wrong Type {}", descr)
        }
        InvalidPacket(descr: &'static str) {
            description(descr)
            display("Invalid Packet {}", descr)
        }
        Base64(err: base64::DecodeError) {
            from()
            description(err.description())
            display("Invalid Base64 {}", err.description())
        }
        Other(err: Box<std::error::Error>) {
            cause(&**err)
            description(err.description())
            display("Error {}", err.description())
        }
    }
}
