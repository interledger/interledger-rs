use bytes::Bytes;
use log::error;
use ring::{
    aead, digest, hmac,
    rand::{SecureRandom, SystemRandom},
};

const NONCE_LENGTH: usize = 12;
static ENCRYPTION_KEY_GENERATION_STRING: &[u8] = b"ilp_store_redis_encryption_key";

use core::sync::atomic;
use secrecy::{DebugSecret, Secret};
use std::ptr;
use zeroize::Zeroize;

#[derive(Debug)]
pub struct EncryptionKey(pub(crate) aead::SealingKey);

#[derive(Debug)]
pub struct DecryptionKey(pub(crate) aead::OpeningKey);

#[derive(Debug)]
pub struct GenerationKey(pub(crate) hmac::SigningKey);

impl DebugSecret for EncryptionKey {}
impl DebugSecret for DecryptionKey {}
impl DebugSecret for GenerationKey {}

impl Zeroize for EncryptionKey {
    fn zeroize(&mut self) {
        // Instead of clearing the memory, we overwrite the key with a
        // slice filled with zeros
        let empty_key = EncryptionKey(aead::SealingKey::new(&aead::AES_256_GCM, &[0; 32]).unwrap());
        volatile_write(self, empty_key);
        atomic_fence();
    }
}

impl Drop for EncryptionKey {
    fn drop(&mut self) {
        self.zeroize()
    }
}

impl Zeroize for DecryptionKey {
    fn zeroize(&mut self) {
        // Instead of clearing the memory, we overwrite the key with a
        // slice filled with zeros
        let empty_key = DecryptionKey(aead::OpeningKey::new(&aead::AES_256_GCM, &[0; 32]).unwrap());
        volatile_write(self, empty_key);
        atomic_fence();
    }
}

impl Drop for DecryptionKey {
    fn drop(&mut self) {
        self.zeroize()
    }
}

impl Zeroize for GenerationKey {
    fn zeroize(&mut self) {
        // Instead of clearing the memory, we overwrite the key with a
        // slice filled with zeros
        let empty_key = GenerationKey(hmac::SigningKey::new(&digest::SHA256, &[0; 32]));
        volatile_write(self, empty_key);
        atomic_fence();
    }
}

impl Drop for GenerationKey {
    fn drop(&mut self) {
        self.zeroize()
    }
}

// this logic is taken from [here](https://github.com/iqlusioninc/crates/blob/develop/zeroize/src/lib.rs#L388-L400)
// Perform a [volatile
// write](https://doc.rust-lang.org/beta/std/ptr/fn.write_volatile.html) to the
// destination. As written in the link, volatile writes are guaranteed to not be
// re-ordered or elided by the compiler, meaning that we can be sure that the
// memory we want to overwrite will be overwritten (and the compiler will not
// avoid that write for whatever reason)
#[inline]
fn volatile_write<T>(dst: &mut T, src: T) {
    unsafe { ptr::write_volatile(dst, src) }
}

/// Use fences to prevent accesses from being reordered before this
/// point, which should hopefully help ensure that all accessors
/// see zeroes after this point.
#[inline]
fn atomic_fence() {
    atomic::compiler_fence(atomic::Ordering::SeqCst);
}

pub fn generate_keys(server_secret: &[u8]) -> (Secret<EncryptionKey>, Secret<DecryptionKey>) {
    let generation_key = GenerationKey(hmac::SigningKey::new(&digest::SHA256, server_secret));
    let encryption_key = Secret::new(EncryptionKey(
        aead::SealingKey::new(
            &aead::AES_256_GCM,
            hmac::sign(&generation_key.0, ENCRYPTION_KEY_GENERATION_STRING).as_ref(),
        )
        .unwrap(),
    ));
    let decryption_key = Secret::new(DecryptionKey(
        aead::OpeningKey::new(
            &aead::AES_256_GCM,
            hmac::sign(&generation_key.0, ENCRYPTION_KEY_GENERATION_STRING).as_ref(),
        )
        .unwrap(),
    ));
    // the generation key is dropped and zeroized here
    (encryption_key, decryption_key)
}

pub fn encrypt_token(encryption_key: &aead::SealingKey, token: &[u8]) -> Bytes {
    let mut token = token.to_vec();
    token.extend_from_slice(&[0u8; 16]);

    let mut nonce: [u8; NONCE_LENGTH] = [0; NONCE_LENGTH];
    SystemRandom::new()
        .fill(&mut nonce)
        .expect("Unable to get sufficient entropy for nonce");
    let nonce_copy = nonce;
    let nonce = aead::Nonce::assume_unique_for_key(nonce);
    if let Ok(out_len) = aead::seal_in_place(
        &encryption_key,
        nonce,
        aead::Aad::from(&[]),
        &mut token,
        encryption_key.algorithm().tag_len(),
    ) {
        token.split_off(out_len);
        token.append(&mut nonce_copy.as_ref().to_vec());
        Bytes::from(token)
    } else {
        panic!("Unable to encrypt token");
    }
}

pub fn decrypt_token(decryption_key: &aead::OpeningKey, encrypted: &[u8]) -> Option<Bytes> {
    if encrypted.len() < aead::MAX_TAG_LEN {
        error!("Cannot decrypt token, encrypted value does not have a nonce attached");
        return None;
    }

    let mut encrypted = encrypted.to_vec();
    let nonce_bytes = encrypted.split_off(encrypted.len() - NONCE_LENGTH);
    let mut nonce: [u8; NONCE_LENGTH] = [0; NONCE_LENGTH];
    nonce.copy_from_slice(nonce_bytes.as_ref());
    let nonce = aead::Nonce::assume_unique_for_key(nonce);

    if let Ok(token) =
        aead::open_in_place(decryption_key, nonce, aead::Aad::empty(), 0, &mut encrypted)
    {
        Some(Bytes::from(token.to_vec()))
    } else {
        error!("Unable to decrypt token");
        None
    }
}

#[cfg(test)]
mod encryption {
    use super::*;
    use secrecy::ExposeSecret;
    use std::str;

    #[test]
    fn encrypts_and_decrypts() {
        let (encryption_key, decryption_key) = generate_keys(&[9; 32]);
        let encrypted = encrypt_token(&encryption_key.expose_secret().0, b"test test");
        let decrypted = decrypt_token(&decryption_key.expose_secret().0, encrypted.as_ref());
        assert_eq!(
            str::from_utf8(decrypted.unwrap().as_ref()).unwrap(),
            "test test"
        );
    }
}
