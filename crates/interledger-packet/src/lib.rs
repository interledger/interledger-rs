//! # interledger-packet
//!
//! Interledger packet serialization/deserialization.

mod address;

mod error;
mod errors;
#[cfg(test)]
mod fixtures;
pub mod hex;
pub mod oer;
mod packet;

pub use self::address::{Address, AddressError};
pub use self::error::{ErrorClass, ErrorCode};
pub use self::errors::ParseError;

pub use self::packet::MaxPacketAmountDetails;
pub use self::packet::{Fulfill, Packet, PacketType, Prepare, Reject};
pub use self::packet::{FulfillBuilder, PrepareBuilder, RejectBuilder};

#[cfg(any(fuzzing, test))]
pub fn lenient_packet_roundtrips(data: &[u8]) -> Result<(), ParseError> {
    use bytes::BytesMut;
    use std::convert::{TryFrom, TryInto};

    let pkt = Packet::try_from(BytesMut::from(data))?;

    // try to create a corresponding builder and a new set of bytes
    match pkt {
        Packet::Prepare(p) => {
            let other = PrepareBuilder {
                amount: p.amount(),
                expires_at: p.expires_at(),
                destination: p.destination(),
                execution_condition: p
                    .execution_condition()
                    .try_into()
                    .expect("wrong length slice"),
                data: p.data(),
            }
            .build();

            if p == other {
                // if the packet roundtripped, great, we are done
                return Ok(());
            }

            // doublecheck to see if this was because fuzzer added bytes in the trailer of the
            // initial varlen field.
            //
            // bytes outside of the outermost varlen field are not accepted per specs.

            assert_eq!(p.amount(), other.amount());
            assert_eq!(p.expires_at(), other.expires_at());
            assert_eq!(p.destination(), other.destination());
            assert_eq!(p.execution_condition(), other.execution_condition());
            assert_eq!(p.data(), other.data());

            let p = BytesMut::from(p);
            let other = BytesMut::from(other);

            // since the components are equal, make sure that the only way the difference can
            // be is with *extra* bytes
            assert!(p.len() > other.len());
        }
        Packet::Fulfill(f) => {
            let other = FulfillBuilder {
                fulfillment: f.fulfillment().try_into().expect("wrong length slice"),
                data: f.data(),
            }
            .build();

            if f == other {
                return Ok(());
            }

            assert_eq!(f.fulfillment(), other.fulfillment());
            assert_eq!(f.data(), other.data());

            let f = BytesMut::from(f);
            let other = BytesMut::from(other);
            assert!(f.len() > other.len());
        }
        Packet::Reject(r) => {
            let other = RejectBuilder {
                code: r.code(),
                message: r.message(),
                triggered_by: r.triggered_by().as_ref(),
                data: r.data(),
            }
            .build();

            if r == other {
                return Ok(());
            }

            assert_eq!(r.code(), other.code());
            assert_eq!(r.message(), other.message());
            assert_eq!(r.triggered_by(), other.triggered_by());
            assert_eq!(r.data(), other.data());

            let r = BytesMut::from(r);
            let other = BytesMut::from(other);
            assert!(r.len() > other.len());
        }
    }

    Ok(())
}
