// TODO make these helpers another crate or something so that we could reuse it.
pub mod redis_test_helpers;

use ring::rand::{SecureRandom, SystemRandom};

pub fn random_secret() -> [u8; 32] {
    let mut bytes: [u8; 32] = [0; 32];
    SystemRandom::new().fill(&mut bytes).unwrap();
    bytes
}
