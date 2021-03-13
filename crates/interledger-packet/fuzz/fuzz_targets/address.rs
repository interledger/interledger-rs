#![no_main]
use libfuzzer_sys::fuzz_target;
use std::convert::TryFrom;

fuzz_target!(|data: &[u8]| {
    let _ = interledger_packet::Address::try_from(data);
});
