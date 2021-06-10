#![no_main]
use interledger_packet::{oer, Address};
use libfuzzer_sys::fuzz_target;
use std::convert::TryFrom;

fuzz_target!(|data: &[u8]| {
    if let Ok(address) = Address::try_from(data) {
        assert!(oer::predict_var_octet_string(address.len()) >= Address::MIN_LEN);
    }
});
