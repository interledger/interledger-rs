use bytes::Bytes;
use ring::{
    aead, digest, hmac,
    rand::{SecureRandom, SystemRandom},
};

const NONCE_LENGTH: usize = 12;
static HMAC_KEY_GENERATION_STRING: &[u8] = b"ilp_store_redis_hmac_key";
static ENCRYPTION_KEY_GENERATION_STRING: &[u8] = b"ilp_store_redis_encryption_key";

pub fn generate_keys(
    server_secret: &[u8],
) -> (hmac::SigningKey, aead::SealingKey, aead::OpeningKey) {
    let generation_key = hmac::SigningKey::new(&digest::SHA256, server_secret);
    let hmac_key = hmac::SigningKey::new(
        &digest::SHA256,
        hmac::sign(&generation_key, HMAC_KEY_GENERATION_STRING).as_ref(),
    );
    let encryption_key = aead::SealingKey::new(
        &aead::AES_256_GCM,
        hmac::sign(&generation_key, ENCRYPTION_KEY_GENERATION_STRING).as_ref(),
    )
    .unwrap();
    let decryption_key = aead::OpeningKey::new(
        &aead::AES_256_GCM,
        hmac::sign(&generation_key, ENCRYPTION_KEY_GENERATION_STRING).as_ref(),
    )
    .unwrap();
    (hmac_key, encryption_key, decryption_key)
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
    use std::str;

    #[test]
    fn encrypts_and_decrypts() {
        let (_, encryption_key, decryption_key) = generate_keys(&[9; 32]);
        let encrypted = encrypt_token(&encryption_key, b"test test");
        let decrypted = decrypt_token(&decryption_key, encrypted.as_ref());
        assert_eq!(
            str::from_utf8(decrypted.unwrap().as_ref()).unwrap(),
            "test test"
        );
    }
}
