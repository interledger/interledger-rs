#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ErrorCode([u8; 3]);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ErrorClass {
    Final,
    Temporary,
    Relative,
    Unknown,
}

impl ErrorCode {
    #[inline]
    pub const fn new(bytes: [u8; 3]) -> Self {
        ErrorCode(bytes)
    }

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

#[cfg(test)]
mod test_error_code {
    use super::*;

    #[test]
    fn test_class() {
        assert_eq!(ErrorCode::F00_BAD_REQUEST.class(), ErrorClass::Final);
        assert_eq!(ErrorCode::T00_INTERNAL_ERROR.class(), ErrorClass::Temporary);
        assert_eq!(ErrorCode::R00_TRANSFER_TIMED_OUT.class(), ErrorClass::Relative);
        assert_eq!(ErrorCode::new(*b"???").class(), ErrorClass::Unknown);
    }
}
