use bytes::{Bytes, BytesMut};
use ring::rand::{SecureRandom, SystemRandom};
use ring::{aead, digest, hmac};

const NONCE_LENGTH: usize = 12;
const AUTH_TAG_LENGTH: usize = 16;

lazy_static! {
  static ref ENCRYPTION_KEY_STRING: &'static [u8] = "ilp_stream_encryption".as_bytes();
  static ref FULFILLMENT_GENERATION_STRING: &'static [u8] = "ilp_stream_fulfillment".as_bytes();
}

pub fn hmac_sha256(key: &[u8], message: &[u8]) -> Bytes {
  let key = hmac::SigningKey::new(&digest::SHA256, key);
  let output = hmac::sign(&key, message);
  Bytes::from(output.as_ref())
}

pub fn generate_fulfillment(shared_secret: &[u8], data: &[u8]) -> Bytes {
  let key = hmac_sha256(shared_secret, &FULFILLMENT_GENERATION_STRING);
  hmac_sha256(&key[..], &data)
}

pub fn fulfillment_to_condition(fulfillment: &[u8]) -> Bytes {
  let output = digest::digest(&digest::SHA256, fulfillment);
  Bytes::from(output.as_ref())
}

pub fn encrypt(shared_secret: &[u8], plaintext: BytesMut) -> BytesMut {
  // Generate a random nonce or IV
  let mut nonce: [u8; NONCE_LENGTH] = [0; NONCE_LENGTH];
  SystemRandom::new().fill(&mut nonce[..]).unwrap();

  encrypt_with_nonce(shared_secret, plaintext, &nonce)
}

fn encrypt_with_nonce(shared_secret: &[u8], mut plaintext: BytesMut, nonce: &[u8]) -> BytesMut {
  let key = hmac_sha256(shared_secret, &ENCRYPTION_KEY_STRING);
  let key = aead::SealingKey::new(&aead::AES_256_GCM, &key).unwrap();

  let additional_data: &[u8] = &vec![];

  // seal_in_place expects the data to have enough room (in length, not just capacity) to append the auth tag
  let auth_tag_place_holder: [u8; AUTH_TAG_LENGTH] = [0; AUTH_TAG_LENGTH];
  plaintext.extend_from_slice(&auth_tag_place_holder[..]);

  aead::seal_in_place(
    &key,
    &nonce[..],
    additional_data,
    plaintext.as_mut(),
    AUTH_TAG_LENGTH,
  ).unwrap_or_else(|err| {
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


pub fn decrypt(shared_secret: &[u8], mut ciphertext: BytesMut) -> BytesMut {
  let key = hmac_sha256(shared_secret, &ENCRYPTION_KEY_STRING);
  let key = aead::OpeningKey::new(&aead::AES_256_GCM, &key).unwrap();

  let nonce = ciphertext.split_to(NONCE_LENGTH);
  let auth_tag = ciphertext.split_to(AUTH_TAG_LENGTH);
  let additional_data: &[u8] = &vec![];

  // Ring expects the tag to come after the data
  ciphertext.unsplit(auth_tag);

  let length = aead::open_in_place(&key, &nonce[..], additional_data, 0, ciphertext.as_mut())
    .unwrap_or_else(|err| {
      error!("Error decrypting {:?}", err);
      panic!(err);
    }).len();
  ciphertext.truncate(length);
  ciphertext
}

#[cfg(test)]
mod fulfillment_and_condition {
  use super::*;

  lazy_static! {
    static ref SHARED_SECRET: Vec<u8> = vec![126,219,117,93,118,248,249,211,20,211,65,110,237,80,253,179,81,146,229,67,231,49,92,127,254,230,144,102,103,166,150,36];
    static ref DATA: Vec<u8> = vec![119,248,213,234,63,200,224,140,212,222,105,159,246,203,66,155,151,172,68,24,76,232,90,10,237,146,189,73,248,196,177,108,115,223];
    static ref FULFILLMENT: Vec<u8> = vec![24,6,56,73,229,236,88,227,82,112,152,49,152,73,182,183,198,7,233,124,119,65,13,68,54,108,120,193,59,226,107,39];
  }

  #[test]
  fn it_generates_the_same_fulfillment_as_javascript() {
    let fulfillment = generate_fulfillment(&SHARED_SECRET, &DATA);
    assert_eq!(fulfillment.to_vec(), *FULFILLMENT);
  }
}

#[cfg(test)]
mod encrypt_decrypt_test {
  use super::*;

  lazy_static! {
    static ref SHARED_SECRET: Vec<u8> = vec![126,219,117,93,118,248,249,211,20,211,65,110,237,80,253,179,81,146,229,67,231,49,92,127,254,230,144,102,103,166,150,36];
    static ref PLAINTEXT: Vec<u8> = vec![99, 0, 12, 255, 77, 31];
    static ref CIPHERTEXT: Vec<u8> = vec![119,248,213,234,63,200,224,140,212,222,105,159,246,203,66,155,151,172,68,24,76,232,90,10,237,146,189,73,248,196,177,108,115,223];
    static ref NONCE: Vec<u8> = vec![119,248,213,234,63,200,224,140,212,222,105,159];
  }

  #[test]
  fn it_encrypts_to_same_as_javascript() {
    let encrypted = encrypt_with_nonce(&SHARED_SECRET, BytesMut::from(&PLAINTEXT[..]), &NONCE[..]);
    assert_eq!(encrypted.to_vec(), *CIPHERTEXT);
  }

  #[test]
  fn it_decrypts_javascript_ciphertext() {
    let decrypted = decrypt(&SHARED_SECRET, BytesMut::from(&CIPHERTEXT[..]));
    assert_eq!(decrypted.to_vec(), *PLAINTEXT);
  }

  #[test]
  fn it_losslessly_encrypts_and_decrypts() {
    let ciphertext = encrypt(&SHARED_SECRET, BytesMut::from(&PLAINTEXT[..]));
    let decrypted = decrypt(&SHARED_SECRET, ciphertext);
    assert_eq!(decrypted.to_vec(), *PLAINTEXT);
  }
}
