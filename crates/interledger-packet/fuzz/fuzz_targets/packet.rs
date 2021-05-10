#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = interledger_packet::lenient_packet_roundtrips(data);
});
