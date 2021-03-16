#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    interledger_stream::fuzz_decrypted_stream_packet(data);
});
