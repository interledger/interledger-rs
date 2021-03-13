#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    interledger_btp::fuzzing::roundtrip_btppacket(data);
});
