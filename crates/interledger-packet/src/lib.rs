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
pub use self::errors::{OerError, PacketTypeError, ParseError, TrailingBytesError};

pub use self::packet::MaxPacketAmountDetails;
pub use self::packet::{Fulfill, Packet, PacketType, Prepare, Reject};
pub use self::packet::{FulfillBuilder, PrepareBuilder, RejectBuilder};

#[cfg(any(fuzzing, test))]
pub fn lenient_packet_roundtrips(data: &[u8]) -> Result<(), ParseError> {
    use bytes::BytesMut;
    use hex::HexString;
    use std::convert::{TryFrom, TryInto};

    let pkt = Packet::try_from(BytesMut::from(data))?;

    // try to create a corresponding builder and a new set of bytes
    let other = match pkt {
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

            BytesMut::from(other)
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

            BytesMut::from(other)
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

            BytesMut::from(other)
        }
    };

    assert_eq!(HexString(data), HexString(&other[..]));

    Ok(())
}
