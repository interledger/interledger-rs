//! ILP address types.
//!
//! Reference: [ILP Addresses - v2.0.0](https://github.com/interledger/rfcs/blob/master/0015-ilp-addresses/0015-ilp-addresses.md).

// Addresses are never empty.
#![allow(clippy::len_without_is_empty)]

use std::borrow::Borrow;
use std::error;
use std::fmt;
use std::str;

use bytes::{BufMut, Bytes, BytesMut};

const MAX_ADDRESS_LENGTH: usize = 1023;

/// An ILP address backed by `Bytes`.
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct Address(Bytes);

impl Address {
    /// # Panics
    ///
    /// Panics if the bytes are not a valid ILP address.
    pub fn new(bytes: &'static [u8]) -> Self {
        Address::try_from(Bytes::from(bytes)).expect("invalid ILP address")
    }

    /// Creates an ILP address without validating the bytes.
    ///
    /// # Safety
    ///
    /// The given bytes must be a valid ILP address.
    #[inline]
    pub const unsafe fn new_unchecked(bytes: Bytes) -> Self {
        Address(bytes)
    }

    pub fn try_from(bytes: Bytes) -> Result<Self, AddressError> {
        Addr::try_from(bytes.as_ref())?;
        Ok(Address(bytes))
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.as_addr().len()
    }

    #[inline]
    pub fn scheme(&self) -> &[u8] {
        self.as_addr().scheme()
    }

    pub fn with_suffix(&self, suffix: &[u8]) -> Result<Self, AddressError> {
        self.as_addr().with_suffix(suffix)
    }

    #[inline]
    pub fn as_addr(&self) -> Addr {
        Addr(self.0.as_ref())
    }
}

impl AsRef<[u8]> for Address {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        self.0.as_ref()
    }
}

impl AsRef<Bytes> for Address {
    #[inline]
    fn as_ref(&self) -> &Bytes {
        &self.0
    }
}

impl fmt::Debug for Address {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.debug_tuple("Address")
            .field(&str::from_utf8(&self.0).unwrap())
            .finish()
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(str::from_utf8(&self.0).unwrap())
    }
}

/// A borrowed ILP address.
///
/// See: <https://github.com/interledger/rfcs/blob/master/0015-ilp-addresses/0015-ilp-addresses.md>
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct Addr<'a>(&'a [u8]);

impl<'a> Addr<'a> {
    /// Creates an ILP address. This test is mostly useful for tests. Generally
    /// [`Addr`]s should be created with `Addr::try_from` instead.
    ///
    /// # Panics
    ///
    /// Panics if the bytes are not a valid ILP address.
    pub fn new(bytes: &'static [u8]) -> Self {
        Addr::try_from(bytes).expect("invalid ILP address")
    }

    /// Creates an ILP address without validating the bytes.
    ///
    /// # Safety
    ///
    /// The given bytes must be a valid ILP address.
    #[inline]
    pub const unsafe fn new_unchecked(bytes: &'a [u8]) -> Self {
        Addr(bytes)
    }

    pub fn try_from(bytes: &'a [u8]) -> Result<Self, AddressError> {
        let mut segments = 0;
        let is_valid = bytes.len() <= MAX_ADDRESS_LENGTH && bytes
            .split(|&byte| byte == b'.')
            .enumerate()
            .all(|(i, segment)| {
                segments += 1;
                let scheme_ok = i != 0 || is_scheme(segment);
                scheme_ok
                    && !segment.is_empty()
                    && segment.iter().all(|&byte| is_segment_byte(byte))
            });
        if is_valid && segments > 1 {
            Ok(Addr(bytes))
        } else {
            Err(AddressError {})
        }
    }

    pub fn to_address(&self) -> Address {
        Address(Bytes::from(self.0))
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// ```text
    /// scheme = "g" / "private" / "example" / "peer" / "self" /
    ///          "test" / "test1" / "test2" / "test3" / "local"
    /// ```
    pub fn scheme(&self) -> &'a [u8] {
        self.0
            .split(|&byte| byte == b'.')
            .next()
            .unwrap()
    }

    pub fn with_suffix(&self, suffix: &[u8]) -> Result<Address, AddressError> {
        let new_address_len = self.len() + 1 + suffix.len();
        let mut new_address = BytesMut::with_capacity(new_address_len);

        new_address.put_slice(self.0.as_ref());
        new_address.put(b'.');
        new_address.put_slice(suffix);

        Address::try_from(new_address.freeze())
    }
}

impl<'a> AsRef<[u8]> for Addr<'a> {
    #[inline]
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl<'a> Borrow<[u8]> for Addr<'a> {
    #[inline]
    fn borrow(&self) -> &[u8] {
        &self.0
    }
}

impl<'a> fmt::Debug for Addr<'a> {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.debug_tuple("Addr")
            .field(&str::from_utf8(&self.0).unwrap())
            .finish()
    }
}

impl<'a> fmt::Display for Addr<'a> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(str::from_utf8(self.0).unwrap())
    }
}

impl<'a> PartialEq<[u8]> for Addr<'a> {
    fn eq(&self, other: &[u8]) -> bool {
        self.0 == other
    }
}

#[cfg(any(feature = "serde", test))]
impl<'de> serde::Deserialize<'de> for Address {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Addr::deserialize(deserializer)
            .map(|addr| Address(Bytes::from(addr.as_ref())))
    }
}

#[cfg(any(feature = "serde", test))]
impl<'de> serde::Deserialize<'de> for Addr<'de> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let string = <&str>::deserialize(deserializer)?;
        Addr::try_from(string.as_bytes())
            .map_err(serde::de::Error::custom)
    }
}

static SCHEMES: &'static [&'static [u8]] = &[
    b"g", b"private", b"example", b"peer", b"self",
    b"test", b"test1", b"test2", b"test3", b"local",
];

fn is_scheme(segment: &[u8]) -> bool {
    SCHEMES.contains(&segment)
}

/// <https://github.com/interledger/rfcs/blob/master/0015-ilp-addresses/0015-ilp-addresses.md#address-requirements>
fn is_segment_byte(byte: u8) -> bool {
    byte == b'_' || byte == b'-' || byte == b'~'
        || (b'A' <= byte && byte <= b'Z')
        || (b'a' <= byte && byte <= b'z')
        || (b'0' <= byte && byte <= b'9')
}

#[derive(Debug)]
pub struct AddressError {}

impl error::Error for AddressError {}

impl fmt::Display for AddressError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("AddressError")
    }
}

#[cfg(test)]
mod test_address {
    use serde_test::{Token, assert_de_tokens, assert_de_tokens_error};

    use super::*;

    #[test]
    fn test_try_from() {
        assert_eq!(
            Address::try_from(Bytes::from("test.alice")).unwrap(),
            Address(Bytes::from("test.alice")),
        );
        assert!(Address::try_from(Bytes::from("test.alice!")).is_err());
    }

    #[test]
    fn test_deserialize() {
        assert_de_tokens(
            &Address::try_from(Bytes::from("test.alice")).unwrap(),
            &[Token::BorrowedStr("test.alice")],
        );
        assert_de_tokens_error::<Address>(
            &[Token::BorrowedStr("test.alice ")],
            "AddressError",
        );
    }

    #[test]
    fn test_debug() {
        assert_eq!(
            format!("{:?}", Address::new(b"test.alice")),
            "Address(\"test.alice\")",
        );
    }

    #[test]
    fn test_display() {
        assert_eq!(
            format!("{}", Address::new(b"test.alice")),
            "test.alice",
        );
    }
}

#[cfg(test)]
mod test_addr {
    use serde_test::{Token, assert_de_tokens, assert_de_tokens_error};

    use super::*;

    static VALID_ADDRESSES: &'static [&'static [u8]] = &[
        b"test.alice.XYZ.1234.-_~",
        b"g.us-fed.ach.0.acmebank.swx0a0.acmecorp.sales.199.~ipr.cdfa5e16-e759-4ba3-88f6-8b9dc83c1868.2",

        b"g.A", b"private.A", b"example.A", b"peer.A", b"self.A",
        b"test.A", b"test1.A", b"test2.A", b"test3.A", b"local.A",
    ];

    static INVALID_ADDRESSES: &'static [&'static [u8]] = &[
        b"", // empty

        // Invalid characters.
        b"test.alice 123",
        b"test.alice!123",
        b"test.alice/123",

        // Bad schemes.
        b"test", // only a scheme
        b"what.alice", // invalid scheme
        b"test4.alice", // invalid scheme

        // Invalid separators.
        b"test.", // only a prefix
        b"test.alice.", // ends in a separator
        b".test.alice", // begins with a separator
        b"test..alice", // double separator
    ];

    #[test]
    fn test_try_from() {
        for address in VALID_ADDRESSES {
            assert_eq!(
                Addr::try_from(address).unwrap(),
                Addr(address),
                "address: {:?}", String::from_utf8_lossy(address),
            );
        }

        let longest_address = &make_address(1023)[..];
        assert_eq!(
            Addr::try_from(longest_address).unwrap(),
            Addr(longest_address),
        );

        for address in INVALID_ADDRESSES {
            assert!(
                Addr::try_from(address).is_err(),
                "address: {:?}", String::from_utf8_lossy(address),
            );
        }

        let too_long_address = &make_address(1024)[..];
        assert!(Addr::try_from(too_long_address).is_err());
    }

    #[test]
    fn test_len() {
        assert_eq!(
            Addr::new(b"test.alice").len(),
            b"test.alice".len(),
        );
    }

    #[test]
    fn test_scheme() {
        assert_eq!(
            Addr::new(b"test.alice").scheme(),
            b"test",
        );
        assert_eq!(
            Addr::new(b"test.alice.1234").scheme(),
            b"test",
        );
    }

    #[test]
    fn test_with_suffix() {
        assert_eq!(
            Addr::new(b"test.alice")
                .with_suffix(b"1234")
                .unwrap(),
            Address::new(b"test.alice.1234"),
        );
        assert!({
            Addr::new(b"test.alice")
                .with_suffix(b"12 34")
                .is_err()
        });
    }

    #[test]
    fn test_deserialize() {
        assert_de_tokens(
            &Addr::new(b"test.alice"),
            &[Token::BorrowedStr("test.alice")],
        );
        assert_de_tokens_error::<Addr>(
            &[Token::BorrowedStr("test.alice ")],
            "AddressError",
        );
    }

    #[test]
    fn test_debug() {
        assert_eq!(
            format!("{:?}", Addr::new(b"test.alice")),
            "Addr(\"test.alice\")",
        );
    }

    #[test]
    fn test_display() {
        assert_eq!(
            format!("{}", Addr::new(b"test.alice")),
            "test.alice",
        );
    }

    fn make_address(length: usize) -> Vec<u8> {
        let mut addr = b"test.".to_vec();
        addr.resize(length, b'_');
        addr
    }
}
