use bytes::BytesMut;
#[cfg(test)]
use lazy_static::lazy_static;
use log::error;
use ring::rand::{SecureRandom, SystemRandom};
use ring::{aead, digest, hmac};

const NONCE_LENGTH: usize = 12;
const AUTH_TAG_LENGTH: usize = 16;

static ENCRYPTION_KEY_STRING: &[u8] = b"ilp_stream_encryption";
static FULFILLMENT_GENERATION_STRING: &[u8] = b"ilp_stream_fulfillment";

pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let key = hmac::Key::new(hmac::HMAC_SHA256, key);
    let output = hmac::sign(&key, message);
    let mut to_return: [u8; 32] = [0; 32];
    to_return.copy_from_slice(output.as_ref());
    to_return
}

pub fn generate_fulfillment(shared_secret: &[u8], data: &[u8]) -> [u8; 32] {
    let key = hmac_sha256(&shared_secret[..], &FULFILLMENT_GENERATION_STRING);
    hmac_sha256(&key[..], &data[..])
}

pub fn hash_sha256(preimage: &[u8]) -> [u8; 32] {
    let output = digest::digest(&digest::SHA256, &preimage[..]);
    let mut to_return: [u8; 32] = [0; 32];
    to_return.copy_from_slice(output.as_ref());
    to_return
}

pub fn generate_condition(shared_secret: &[u8], data: &[u8]) -> [u8; 32] {
    let fulfillment = generate_fulfillment(&shared_secret, &data);
    hash_sha256(&fulfillment)
}

pub fn random_condition() -> [u8; 32] {
    let mut condition_slice: [u8; 32] = [0; 32];
    SystemRandom::new()
        .fill(&mut condition_slice)
        .expect("Failed to securely generate random condition!");
    condition_slice
}

pub fn generate_token() -> [u8; 18] {
    let mut token: [u8; 18] = [0; 18];
    SystemRandom::new()
        .fill(&mut token)
        .expect("Failed to securely generate a random token!");
    token
}

pub fn encrypt(shared_secret: &[u8], plaintext: BytesMut) -> BytesMut {
    // Generate a random nonce or IV
    let mut nonce: [u8; NONCE_LENGTH] = [0; NONCE_LENGTH];
    SystemRandom::new()
        .fill(&mut nonce[..])
        .expect("Failed to securely generate a random nonce!");

    encrypt_with_nonce(shared_secret, plaintext, nonce)
}

fn encrypt_with_nonce(
    shared_secret: &[u8],
    mut plaintext: BytesMut,
    nonce: [u8; NONCE_LENGTH],
) -> BytesMut {
    let key = hmac_sha256(&shared_secret[..], &ENCRYPTION_KEY_STRING);
    let key = aead::UnboundKey::new(&aead::AES_256_GCM, &key)
        .expect("Failed to create a new sealing key for encrypting data!");
    let key = aead::LessSafeKey::new(key);

    let additional_data = aead::Aad::from(&[]);

    key.seal_in_place_append_tag(
        aead::Nonce::assume_unique_for_key(nonce),
        additional_data,
        &mut plaintext,
    )
    .unwrap_or_else(|err| {
        error!("Error encrypting {:?}", err);
        panic!(err);
    });

    // Rearrange the bytes so that the tag goes first (should have put it last in the JS implementation, but oh well)
    let auth_tag_position = plaintext.len() - AUTH_TAG_LENGTH;
    let mut tag_data = plaintext.split_off(auth_tag_position);
    tag_data.unsplit(plaintext);

    // The format is `nonce, auth tag, data`, in that order
    let mut nonce_tag_data = BytesMut::from(&nonce[..]);
    nonce_tag_data.unsplit(tag_data);

    nonce_tag_data
}

pub fn decrypt(shared_secret: &[u8], mut ciphertext: BytesMut) -> Result<BytesMut, ()> {
    // ciphertext must include at least a nonce and tag,
    if ciphertext.len() < AUTH_TAG_LENGTH {
        return Err(());
    }

    let key = hmac_sha256(shared_secret, &ENCRYPTION_KEY_STRING);
    let key = aead::UnboundKey::new(&aead::AES_256_GCM, &key)
        .expect("Failed to create a new opening key for decrypting data!");
    let key = aead::LessSafeKey::new(key);

    let mut nonce: [u8; NONCE_LENGTH] = [0; NONCE_LENGTH];
    nonce.copy_from_slice(&ciphertext.split_to(NONCE_LENGTH));

    let auth_tag = ciphertext.split_to(AUTH_TAG_LENGTH);
    let additional_data: &[u8] = &[];

    // Ring expects the tag to come after the data
    ciphertext.unsplit(auth_tag);

    let length = key
        .open_in_place(
            aead::Nonce::assume_unique_for_key(nonce),
            aead::Aad::from(additional_data),
            &mut ciphertext,
        )
        .map_err(|err| {
            error!("Error decrypting {:?}", err);
        })?
        .len();
    ciphertext.truncate(length);
    Ok(ciphertext)
}

#[cfg(test)]
mod fulfillment_and_condition {
    use super::*;
    use bytes::Bytes;

    lazy_static! {
        static ref SHARED_SECRET: Vec<u8> = vec![
            126, 219, 117, 93, 118, 248, 249, 211, 20, 211, 65, 110, 237, 80, 253, 179, 81, 146,
            229, 67, 231, 49, 92, 127, 254, 230, 144, 102, 103, 166, 150, 36
        ];
        static ref DATA: Vec<u8> = vec![
            119, 248, 213, 234, 63, 200, 224, 140, 212, 222, 105, 159, 246, 203, 66, 155, 151, 172,
            68, 24, 76, 232, 90, 10, 237, 146, 189, 73, 248, 196, 177, 108, 115, 223
        ];
        static ref FULFILLMENT: Vec<u8> = vec![
            24, 6, 56, 73, 229, 236, 88, 227, 82, 112, 152, 49, 152, 73, 182, 183, 198, 7, 233,
            124, 119, 65, 13, 68, 54, 108, 120, 193, 59, 226, 107, 39
        ];
    }

    #[test]
    fn it_generates_the_same_fulfillment_as_javascript() {
        let fulfillment =
            generate_fulfillment(&Bytes::from(&SHARED_SECRET[..]), &Bytes::from(&DATA[..]));
        assert_eq!(fulfillment.to_vec(), *FULFILLMENT);
    }
}

#[cfg(test)]
mod encrypt_decrypt_test {
    use super::*;

    static SHARED_SECRET: &[u8] = &[
        126, 219, 117, 93, 118, 248, 249, 211, 20, 211, 65, 110, 237, 80, 253, 179, 81, 146, 229,
        67, 231, 49, 92, 127, 254, 230, 144, 102, 103, 166, 150, 36,
    ];
    static PLAINTEXT: &[u8] = &[99, 0, 12, 255, 77, 31];
    static CIPHERTEXT: &[u8] = &[
        119, 248, 213, 234, 63, 200, 224, 140, 212, 222, 105, 159, 246, 203, 66, 155, 151, 172, 68,
        24, 76, 232, 90, 10, 237, 146, 189, 73, 248, 196, 177, 108, 115, 223,
    ];
    static NONCE: [u8; NONCE_LENGTH] = [119, 248, 213, 234, 63, 200, 224, 140, 212, 222, 105, 159];

    #[test]
    fn it_encrypts_to_same_as_javascript() {
        let encrypted =
            encrypt_with_nonce(&SHARED_SECRET[..], BytesMut::from(&PLAINTEXT[..]), NONCE);
        assert_eq!(&encrypted[..], CIPHERTEXT);
    }

    #[test]
    fn it_decrypts_javascript_ciphertext() {
        let decrypted = decrypt(SHARED_SECRET, BytesMut::from(CIPHERTEXT));
        assert_eq!(&decrypted.unwrap()[..], PLAINTEXT);
    }

    #[test]
    fn it_losslessly_encrypts_and_decrypts() {
        let ciphertext = encrypt(SHARED_SECRET, BytesMut::from(PLAINTEXT));
        let decrypted = decrypt(SHARED_SECRET, ciphertext);
        assert_eq!(&decrypted.unwrap()[..], PLAINTEXT);
    }
}
