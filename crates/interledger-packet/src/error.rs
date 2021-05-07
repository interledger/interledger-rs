use std::fmt;
use std::str;

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct ErrorCode([u8; 3]);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorClass {
    Final,
    Temporary,
    Relative,
    Unknown,
}

impl ErrorCode {
    /// Returns a `Some(ErrorCode)` value if the given bytes are IA5String or 7-bit ascii
    pub fn new(bytes: [u8; 3]) -> Option<Self> {
        if bytes.iter().all(|&b| b < 128) {
            // asn.1 defintion says IA5String which means 7-bit ascii
            // https://github.com/interledger/rfcs/blob/2473d2963a65e5534076c483f3c08a81b8e0cc88/asn1/InterledgerProtocol.asn#L43
            Some(ErrorCode(bytes))
        } else {
            None
        }
    }

    #[inline]
    pub fn class(self) -> ErrorClass {
        match self.0[0] {
            b'F' => ErrorClass::Final,
            b'T' => ErrorClass::Temporary,
            b'R' => ErrorClass::Relative,
            _ => ErrorClass::Unknown,
        }
    }

    // Error codes from: <https://github.com/interledger/rfcs/blob/master/0027-interledger-protocol-4/0027-interledger-protocol-4.md#error-codes>

    // Final errors:
    pub const F00_BAD_REQUEST: Self = ErrorCode(*b"F00");
    pub const F01_INVALID_PACKET: Self = ErrorCode(*b"F01");
    pub const F02_UNREACHABLE: Self = ErrorCode(*b"F02");
    pub const F03_INVALID_AMOUNT: Self = ErrorCode(*b"F03");
    pub const F04_INSUFFICIENT_DESTINATION_AMOUNT: Self = ErrorCode(*b"F04");
    pub const F05_WRONG_CONDITION: Self = ErrorCode(*b"F05");
    pub const F06_UNEXPECTED_PAYMENT: Self = ErrorCode(*b"F06");
    pub const F07_CANNOT_RECEIVE: Self = ErrorCode(*b"F07");
    pub const F08_AMOUNT_TOO_LARGE: Self = ErrorCode(*b"F08");
    pub const F09_INVALID_PEER_RESPONSE: Self = ErrorCode(*b"F09");
    pub const F99_APPLICATION_ERROR: Self = ErrorCode(*b"F99");

    // Temporary errors:
    pub const T00_INTERNAL_ERROR: Self = ErrorCode(*b"T00");
    pub const T01_PEER_UNREACHABLE: Self = ErrorCode(*b"T01");
    pub const T02_PEER_BUSY: Self = ErrorCode(*b"T02");
    pub const T03_CONNECTOR_BUSY: Self = ErrorCode(*b"T03");
    pub const T04_INSUFFICIENT_LIQUIDITY: Self = ErrorCode(*b"T04");
    pub const T05_RATE_LIMITED: Self = ErrorCode(*b"T05");
    pub const T99_APPLICATION_ERROR: Self = ErrorCode(*b"T99");

    // Relative errors:
    pub const R00_TRANSFER_TIMED_OUT: Self = ErrorCode(*b"R00");
    pub const R01_INSUFFICIENT_SOURCE_AMOUNT: Self = ErrorCode(*b"R01");
    pub const R02_INSUFFICIENT_TIMEOUT: Self = ErrorCode(*b"R02");
    pub const R99_APPLICATION_ERROR: Self = ErrorCode(*b"R99");
}

impl From<ErrorCode> for [u8; 3] {
    fn from(error_code: ErrorCode) -> Self {
        error_code.0
    }
}

impl fmt::Debug for ErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        let error_str = match *self {
            ErrorCode::F00_BAD_REQUEST => "F00 (Bad Request)",
            ErrorCode::F01_INVALID_PACKET => "F01 (Invalid Packet)",
            ErrorCode::F02_UNREACHABLE => "F02 (Unreachable)",
            ErrorCode::F03_INVALID_AMOUNT => "F03 (Invalid Amount)",
            ErrorCode::F04_INSUFFICIENT_DESTINATION_AMOUNT => {
                "F04 (Insufficient Destination Amount)"
            }
            ErrorCode::F05_WRONG_CONDITION => "F05 (Wrong Condition)",
            ErrorCode::F06_UNEXPECTED_PAYMENT => "F06 (Unexpected Payment)",
            ErrorCode::F07_CANNOT_RECEIVE => "F07 (Cannot Receive)",
            ErrorCode::F08_AMOUNT_TOO_LARGE => "F08 (Amount Too Large)",
            ErrorCode::F09_INVALID_PEER_RESPONSE => "F09 (Invalid Peer Response)",
            ErrorCode::F99_APPLICATION_ERROR => "F99 (Application Error)",
            ErrorCode::T00_INTERNAL_ERROR => "T00 (Internal Error)",
            ErrorCode::T01_PEER_UNREACHABLE => "T01 (Peer Unreachable)",
            ErrorCode::T02_PEER_BUSY => "T02 (Peer Busy)",
            ErrorCode::T03_CONNECTOR_BUSY => "T03 (Connector Busy)",
            ErrorCode::T04_INSUFFICIENT_LIQUIDITY => "T04 (Insufficient Liquidity)",
            ErrorCode::T05_RATE_LIMITED => "T05 (Rate Limited)",
            ErrorCode::T99_APPLICATION_ERROR => "T99 (Application Error)",
            ErrorCode::R00_TRANSFER_TIMED_OUT => "R00 (Transfer Timed Out)",
            ErrorCode::R01_INSUFFICIENT_SOURCE_AMOUNT => "R01 (Insufficient Source Amount)",
            ErrorCode::R02_INSUFFICIENT_TIMEOUT => "R02 (Insufficient Timeout)",
            ErrorCode::R99_APPLICATION_ERROR => "R99 (Application Error)",
            _ => {
                str::from_utf8(&self.0[..]).expect("ErrorCode::new accepts only IA5String or ascii")
            }
        };
        formatter
            .debug_tuple("ErrorCode")
            .field(&error_str)
            .finish()
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        let as_str =
            str::from_utf8(&self.0[..]).expect("ErrorCode::new accepts only IA5String or ascii");
        if as_str.chars().any(|c| c.is_ascii_control()) {
            // escape control characters not to garbage any raw output readers
            write!(formatter, "{:?}", as_str)
        } else {
            formatter.write_str(as_str)
        }
    }
}

#[cfg(test)]
mod test_error_code {
    use super::*;

    #[test]
    fn test_class() {
        assert_eq!(ErrorCode::F00_BAD_REQUEST.class(), ErrorClass::Final);
        assert_eq!(ErrorCode::T00_INTERNAL_ERROR.class(), ErrorClass::Temporary);
        assert_eq!(
            ErrorCode::R00_TRANSFER_TIMED_OUT.class(),
            ErrorClass::Relative
        );
        assert_eq!(
            ErrorCode::new(*b"???")
                .expect("questionmarks are accepted")
                .class(),
            ErrorClass::Unknown
        );
    }

    #[test]
    fn rejects_non_ia5string() {
        use std::convert::TryInto;
        let bytes = "ä1".as_bytes().try_into().unwrap();
        assert_eq!(ErrorCode::new(bytes), None);
    }

    #[test]
    fn control_characters_escaped() {
        let bogus = ErrorCode::new(*b"\x00\x01\x02").unwrap();
        assert_eq!(&bogus.to_string(), "\"\\u{0}\\u{1}\\u{2}\"");
        assert_eq!(&format!("{:?}", bogus), "ErrorCode(\"\\u{0}\\u{1}\\u{2}\")");

        let good = ErrorCode::new(*b"T01").unwrap();
        assert_eq!(&good.to_string(), "T01");
    }

    #[test]
    fn test_debug_printing() {
        assert_eq!(
            format!("{:?}", ErrorCode::F00_BAD_REQUEST),
            String::from("ErrorCode(\"F00 (Bad Request)\")")
        );
        assert_eq!(
            format!("{:?}", ErrorCode::F01_INVALID_PACKET),
            String::from("ErrorCode(\"F01 (Invalid Packet)\")")
        );
        assert_eq!(
            format!("{:?}", ErrorCode::F02_UNREACHABLE),
            String::from("ErrorCode(\"F02 (Unreachable)\")")
        );
        assert_eq!(
            format!("{:?}", ErrorCode::F03_INVALID_AMOUNT),
            String::from("ErrorCode(\"F03 (Invalid Amount)\")")
        );
        assert_eq!(
            format!("{:?}", ErrorCode::F04_INSUFFICIENT_DESTINATION_AMOUNT),
            String::from("ErrorCode(\"F04 (Insufficient Destination Amount)\")")
        );
        assert_eq!(
            format!("{:?}", ErrorCode::F05_WRONG_CONDITION),
            String::from("ErrorCode(\"F05 (Wrong Condition)\")")
        );
        assert_eq!(
            format!("{:?}", ErrorCode::F06_UNEXPECTED_PAYMENT),
            String::from("ErrorCode(\"F06 (Unexpected Payment)\")")
        );
        assert_eq!(
            format!("{:?}", ErrorCode::F07_CANNOT_RECEIVE),
            String::from("ErrorCode(\"F07 (Cannot Receive)\")")
        );
        assert_eq!(
            format!("{:?}", ErrorCode::F08_AMOUNT_TOO_LARGE),
            String::from("ErrorCode(\"F08 (Amount Too Large)\")")
        );
        assert_eq!(
            format!("{:?}", ErrorCode::F09_INVALID_PEER_RESPONSE),
            String::from("ErrorCode(\"F09 (Invalid Peer Response)\")")
        );
        assert_eq!(
            format!("{:?}", ErrorCode::F99_APPLICATION_ERROR),
            String::from("ErrorCode(\"F99 (Application Error)\")")
        );
        assert_eq!(
            format!("{:?}", ErrorCode::T00_INTERNAL_ERROR),
            String::from("ErrorCode(\"T00 (Internal Error)\")")
        );
        assert_eq!(
            format!("{:?}", ErrorCode::T01_PEER_UNREACHABLE),
            String::from("ErrorCode(\"T01 (Peer Unreachable)\")")
        );
        assert_eq!(
            format!("{:?}", ErrorCode::T02_PEER_BUSY),
            String::from("ErrorCode(\"T02 (Peer Busy)\")")
        );
        assert_eq!(
            format!("{:?}", ErrorCode::T03_CONNECTOR_BUSY),
            String::from("ErrorCode(\"T03 (Connector Busy)\")")
        );
        assert_eq!(
            format!("{:?}", ErrorCode::T04_INSUFFICIENT_LIQUIDITY),
            String::from("ErrorCode(\"T04 (Insufficient Liquidity)\")")
        );
        assert_eq!(
            format!("{:?}", ErrorCode::T05_RATE_LIMITED),
            String::from("ErrorCode(\"T05 (Rate Limited)\")")
        );
        assert_eq!(
            format!("{:?}", ErrorCode::T99_APPLICATION_ERROR),
            String::from("ErrorCode(\"T99 (Application Error)\")")
        );
        assert_eq!(
            format!("{:?}", ErrorCode::R00_TRANSFER_TIMED_OUT),
            String::from("ErrorCode(\"R00 (Transfer Timed Out)\")")
        );
        assert_eq!(
            format!("{:?}", ErrorCode::R01_INSUFFICIENT_SOURCE_AMOUNT),
            String::from("ErrorCode(\"R01 (Insufficient Source Amount)\")")
        );
        assert_eq!(
            format!("{:?}", ErrorCode::R02_INSUFFICIENT_TIMEOUT),
            String::from("ErrorCode(\"R02 (Insufficient Timeout)\")")
        );
        assert_eq!(
            format!("{:?}", ErrorCode::R99_APPLICATION_ERROR),
            String::from("ErrorCode(\"R99 (Application Error)\")")
        );
    }

    #[test]
    fn test_final_error_values() {
        assert_eq!(
            format!("{}", ErrorCode::F00_BAD_REQUEST),
            String::from("F00")
        );
        assert_eq!(
            format!("{}", ErrorCode::F01_INVALID_PACKET),
            String::from("F01")
        );
        assert_eq!(
            format!("{}", ErrorCode::F02_UNREACHABLE),
            String::from("F02")
        );
        assert_eq!(
            format!("{}", ErrorCode::F03_INVALID_AMOUNT),
            String::from("F03")
        );
        assert_eq!(
            format!("{}", ErrorCode::F04_INSUFFICIENT_DESTINATION_AMOUNT),
            String::from("F04")
        );
        assert_eq!(
            format!("{}", ErrorCode::F05_WRONG_CONDITION),
            String::from("F05")
        );
        assert_eq!(
            format!("{}", ErrorCode::F06_UNEXPECTED_PAYMENT),
            String::from("F06")
        );
        assert_eq!(
            format!("{}", ErrorCode::F07_CANNOT_RECEIVE),
            String::from("F07")
        );
        assert_eq!(
            format!("{}", ErrorCode::F08_AMOUNT_TOO_LARGE),
            String::from("F08")
        );
        assert_eq!(
            format!("{}", ErrorCode::F09_INVALID_PEER_RESPONSE),
            String::from("F09")
        );
        assert_eq!(
            format!("{}", ErrorCode::F99_APPLICATION_ERROR),
            String::from("F99")
        );
    }

    #[test]
    fn test_temporary_error_values() {
        assert_eq!(
            format!("{}", ErrorCode::T00_INTERNAL_ERROR),
            String::from("T00")
        );
        assert_eq!(
            format!("{}", ErrorCode::T01_PEER_UNREACHABLE),
            String::from("T01")
        );
        assert_eq!(format!("{}", ErrorCode::T02_PEER_BUSY), String::from("T02"));
        assert_eq!(
            format!("{}", ErrorCode::T03_CONNECTOR_BUSY),
            String::from("T03")
        );
        assert_eq!(
            format!("{}", ErrorCode::T04_INSUFFICIENT_LIQUIDITY),
            String::from("T04")
        );
        assert_eq!(
            format!("{}", ErrorCode::T05_RATE_LIMITED),
            String::from("T05")
        );
        assert_eq!(
            format!("{}", ErrorCode::T99_APPLICATION_ERROR),
            String::from("T99")
        );
    }

    #[test]
    fn test_relative_error_values() {
        assert_eq!(
            format!("{}", ErrorCode::R00_TRANSFER_TIMED_OUT),
            String::from("R00")
        );
        assert_eq!(
            format!("{}", ErrorCode::R01_INSUFFICIENT_SOURCE_AMOUNT),
            String::from("R01")
        );
        assert_eq!(
            format!("{}", ErrorCode::R02_INSUFFICIENT_TIMEOUT),
            String::from("R02")
        );
        assert_eq!(
            format!("{}", ErrorCode::R99_APPLICATION_ERROR),
            String::from("R99")
        );
    }
}
