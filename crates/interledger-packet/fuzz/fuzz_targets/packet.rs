#![no_main]
use bytes::BytesMut;
use libfuzzer_sys::fuzz_target;
use std::convert::TryFrom;

fuzz_target!(|data: &[u8]| {
    // no point in roundtrip fuzzing this one, as it would just give out the internal BytesMut
    let _ = interledger_packet::Packet::try_from(BytesMut::from(data));
});
