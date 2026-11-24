//! Station keys. The config keeps only a key's hash; the station app keeps the key.

use crate::ServerError;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::digest::{SHA256, SHA256_OUTPUT_LEN, digest};
use ring::rand::{SecureRandom, SystemRandom};

const HASH_PREFIX: &str = "sha256:";

/// A new station key: 32 random bytes, base64url without padding.
pub fn new_key() -> Result<String, ServerError> {
    let mut bytes = [0u8; 32];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| ServerError::Config("the system gave no random bytes for a key".into()))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

/// The hash the config keeps for `key`: `sha256:` and the lowercase hex of its SHA-256.
pub fn hash_key(key: &str) -> String {
    let mut out = String::with_capacity(HASH_PREFIX.len() + SHA256_OUTPUT_LEN * 2);
    out.push_str(HASH_PREFIX);
    for byte in digest(&SHA256, key.as_bytes()).as_ref() {
        out.push(hex_digit(byte >> 4));
        out.push(hex_digit(byte & 0x0f));
    }
    out
}

/// Whether `key` hashes to `key_hash`, compared in constant time. A hash that doesn't parse
/// matches no key.
pub fn key_matches(key: &str, key_hash: &str) -> bool {
    let Some(expected) = parse_hash(key_hash) else {
        return false;
    };
    let actual = digest(&SHA256, key.as_bytes());
    let difference = actual
        .as_ref()
        .iter()
        .zip(expected.iter())
        .fold(0u8, |acc, (a, b)| acc | (a ^ b));
    difference == 0
}

fn parse_hash(key_hash: &str) -> Option<[u8; SHA256_OUTPUT_LEN]> {
    let hex = key_hash.strip_prefix(HASH_PREFIX)?.as_bytes();
    if hex.len() != SHA256_OUTPUT_LEN * 2 {
        return None;
    }
    let mut out = [0u8; SHA256_OUTPUT_LEN];
    let (pairs, _) = hex.as_chunks::<2>();
    for (byte, [high, low]) in out.iter_mut().zip(pairs) {
        *byte = (hex_value(*high)? << 4) | hex_value(*low)?;
    }
    Some(out)
}

fn hex_digit(nibble: u8) -> char {
    char::from_digit(u32::from(nibble & 0x0f), 16).unwrap_or('0')
}

fn hex_value(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}
