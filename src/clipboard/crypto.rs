//! Encryption at rest: XChaCha20-Poly1305 with a random 256-bit key that is
//! kept only in the Secret Service keyring.

use std::collections::HashMap;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};

const SERVICE: &str = "com.korora.Rockhopper";
const KEY_USER: &str = "clipboard-key";
const MAGIC: &[u8; 4] = b"RHC1";
const NONCE_LEN: usize = 24;

pub struct Cipher(XChaCha20Poly1305);

impl Cipher {
    pub fn new(key: &[u8; 32]) -> Self {
        Self(XChaCha20Poly1305::new(key.into()))
    }

    /// The key from the keyring, created there on first use. `None` when
    /// there is no usable keyring, in which case nothing may be written.
    pub fn from_keyring() -> Option<Self> {
        match keyring_key() {
            Ok(key) => Some(Self::new(&key)),
            Err(err) => {
                eprintln!(
                    "rockhopper: keyring unavailable, clipboard history stays in memory: {err}"
                );
                None
            }
        }
    }

    /// `aad` binds the ciphertext to its file name, so files cannot be
    /// swapped for one another.
    pub fn seal(&self, plain: &[u8], aad: &[u8]) -> Vec<u8> {
        let mut nonce = [0u8; NONCE_LEN];
        getrandom::fill(&mut nonce).expect("system random source");
        let sealed = self
            .0
            .encrypt(XNonce::from_slice(&nonce), Payload { msg: plain, aad })
            .expect("XChaCha20-Poly1305 encryption cannot fail for in-memory input");
        let mut out = Vec::with_capacity(MAGIC.len() + NONCE_LEN + sealed.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&sealed);
        out
    }

    /// `None` when the data is not ours, was sealed with another key or was
    /// tampered with.
    pub fn open(&self, data: &[u8], aad: &[u8]) -> Option<Vec<u8>> {
        let rest = data.strip_prefix(MAGIC)?;
        if rest.len() < NONCE_LEN {
            return None;
        }
        let (nonce, sealed) = rest.split_at(NONCE_LEN);
        self.0
            .decrypt(XNonce::from_slice(nonce), Payload { msg: sealed, aad })
            .ok()
    }
}

fn keyring_key() -> Result<[u8; 32], String> {
    let store = zbus_secret_service_keyring_store::Store::new().map_err(|e| e.to_string())?;
    keyring_core::set_default_store(store);
    let entry = keyring_core::Entry::new_with_modifiers(
        SERVICE,
        KEY_USER,
        &HashMap::from([("label", "Rockhopper: clipboard history key")]),
    )
    .map_err(|e| e.to_string())?;
    match entry.get_password() {
        Ok(hex) => decode_hex(&hex).ok_or_else(|| "the stored key is malformed".into()),
        Err(keyring_core::Error::NoEntry) => {
            let mut key = [0u8; 32];
            getrandom::fill(&mut key).map_err(|e| e.to_string())?;
            entry
                .set_password(&encode_hex(&key))
                .map_err(|e| e.to_string())?;
            Ok(key)
        }
        Err(e) => Err(e.to_string()),
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn decode_hex(hex: &str) -> Option<[u8; 32]> {
    let hex = hex.trim();
    if hex.len() != 64 {
        return None;
    }
    let mut key = [0u8; 32];
    for (i, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_tamper() {
        let cipher = Cipher::new(&[7; 32]);
        let sealed = cipher.seal(b"secret text", b"1.meta");
        assert!(!sealed.windows(6).any(|w| w == b"secret"));
        assert_eq!(cipher.open(&sealed, b"1.meta").unwrap(), b"secret text");
        assert!(
            cipher.open(&sealed, b"2.meta").is_none(),
            "bound to its name"
        );
        assert!(
            Cipher::new(&[8; 32]).open(&sealed, b"1.meta").is_none(),
            "other key"
        );
        let mut flipped = sealed.clone();
        *flipped.last_mut().unwrap() ^= 1;
        assert!(cipher.open(&flipped, b"1.meta").is_none(), "tampered");
        assert!(cipher.open(b"RHC1short", b"1.meta").is_none());
    }

    #[test]
    fn hex() {
        let key = [0xab; 32];
        assert_eq!(decode_hex(&encode_hex(&key)), Some(key));
        assert_eq!(decode_hex("zz"), None);
    }
}
