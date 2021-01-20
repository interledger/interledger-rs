use bytes::BytesMut;
use ring::{
    aead, hmac,
    rand::{SecureRandom, SystemRandom},
};

const NONCE_LENGTH: usize = 12;
static ENCRYPTION_KEY_GENERATION_STRING: &[u8] = b"ilp_store_redis_encryption_key";

use core::sync::atomic;
use secrecy::{DebugSecret, Secret, SecretBytesMut};
use std::ptr;
use zeroize::Zeroize;

#[derive(Debug)]
pub struct EncryptionKey(pub(crate) aead::LessSafeKey);

#[derive(Debug)]
pub struct DecryptionKey(pub(crate) aead::LessSafeKey);

#[derive(Debug)]
pub struct GenerationKey(pub(crate) hmac::Key);

impl DebugSecret for EncryptionKey {}
impl DebugSecret for DecryptionKey {}
impl DebugSecret for GenerationKey {}

impl Zeroize for EncryptionKey {
    fn zeroize(&mut self) {
        // Instead of clearing the memory, we overwrite the key with a
        // slice filled with zeros
        let empty_key = EncryptionKey(aead::LessSafeKey::new(
            aead::UnboundKey::new(&aead::AES_256_GCM, &[0; 32]).unwrap(),
        ));
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
        let empty_key = DecryptionKey(aead::LessSafeKey::new(
            aead::UnboundKey::new(&aead::AES_256_GCM, &[0; 32]).unwrap(),
        ));
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
        let empty_key = GenerationKey(hmac::Key::new(hmac::HMAC_SHA256, &[0; 32]));
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
    let generation_key = GenerationKey(hmac::Key::new(hmac::HMAC_SHA256, server_secret));
    let encryption_key = Secret::new(EncryptionKey(aead::LessSafeKey::new(
        aead::UnboundKey::new(
            &aead::AES_256_GCM,
            hmac::sign(&generation_key.0, ENCRYPTION_KEY_GENERATION_STRING).as_ref(),
        )
        .unwrap(),
    )));
    let decryption_key = Secret::new(DecryptionKey(aead::LessSafeKey::new(
        aead::UnboundKey::new(
            &aead::AES_256_GCM,
            hmac::sign(&generation_key.0, ENCRYPTION_KEY_GENERATION_STRING).as_ref(),
        )
        .unwrap(),
    )));
    // the generation key is dropped and zeroized here
    (encryption_key, decryption_key)
}

pub fn encrypt_token(encryption_key: &aead::LessSafeKey, token: &[u8]) -> BytesMut {
    let mut token = token.to_vec();

    let mut nonce: [u8; NONCE_LENGTH] = [0; NONCE_LENGTH];
    SystemRandom::new()
        .fill(&mut nonce)
        .expect("Unable to get sufficient entropy for nonce");
    let nonce_copy = nonce;
    let nonce = aead::Nonce::assume_unique_for_key(nonce);
    match encryption_key.seal_in_place_append_tag(nonce, aead::Aad::from(&[]), &mut token) {
        Ok(_) => {
            token.append(&mut nonce_copy.as_ref().to_vec());
            BytesMut::from(token.as_slice())
        }
        _ => panic!("Unable to encrypt token"),
    }
}

pub fn decrypt_token(
    decryption_key: &aead::LessSafeKey,
    encrypted: &[u8],
) -> Result<SecretBytesMut, ()> {
    if encrypted.len() < aead::MAX_TAG_LEN {
        return Err(());
    }

    let mut encrypted = encrypted.to_vec();
    let nonce_bytes = encrypted.split_off(encrypted.len() - NONCE_LENGTH);
    let mut nonce: [u8; NONCE_LENGTH] = [0; NONCE_LENGTH];
    nonce.copy_from_slice(nonce_bytes.as_ref());
    let nonce = aead::Nonce::assume_unique_for_key(nonce);

    if let Ok(token) = decryption_key.open_in_place(nonce, aead::Aad::empty(), &mut encrypted) {
        Ok(SecretBytesMut::new(&token[..]))
    } else {
        Err(())
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
            str::from_utf8(decrypted.unwrap().expose_secret().as_ref()).unwrap(),
            "test test"
        );
    }
}
