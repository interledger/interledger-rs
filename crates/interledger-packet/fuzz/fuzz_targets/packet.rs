#![no_main]
use bytes::BytesMut;
use libfuzzer_sys::fuzz_target;
use std::convert::{TryFrom, TryInto};

fuzz_target!(|data: &[u8]| {
    use interledger_packet::{FulfillBuilder, Packet, PrepareBuilder, RejectBuilder};

    if let Ok(pkt) = interledger_packet::Packet::try_from(BytesMut::from(data)) {
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

                assert_eq!(p, other);
            }
            Packet::Fulfill(f) => {
                let other = FulfillBuilder {
                    fulfillment: f.fulfillment().try_into().expect("wrong length slice"),
                    data: f.data(),
                }
                .build();

                // FIXME: it could be that trailing bytes do not error?
                assert_eq!(f, other);
            }
            Packet::Reject(r) => {
                let other = RejectBuilder {
                    code: r.code(),
                    message: r.message(),
                    triggered_by: r.triggered_by().as_ref(),
                    data: r.data(),
                }
                .build();

                assert_eq!(r, other);
            }
        }
    }
});
