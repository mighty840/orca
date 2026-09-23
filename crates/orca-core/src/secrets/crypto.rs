//! Value encryption and master-key handling for the file-backed secret store.
//!
//! Everything here is fallible rather than panicking (#196). A wrong or
//! damaged key, or a malformed value, becomes an error the caller can report.
//! Previously it could panic in `hex_decode` or `Key::from_slice`, or be
//! mistaken for a legacy value and "migrated" into garbage.

use std::fmt::Write;
use std::path::Path;

use aes_gcm::aead::{Aead, OsRng};
use aes_gcm::{AeadCore, Aes256Gcm, Key, KeyInit, Nonce};
use anyhow::{Context, Result, bail, ensure};

/// AES-256 key length in bytes.
pub(super) const KEY_LEN: usize = 32;
/// AES-GCM nonce length in bytes.
const NONCE_LEN: usize = 12;

/// Hex-encode bytes to a string.
pub(super) fn hex_encode(data: &[u8]) -> String {
    let mut s = String::with_capacity(data.len() * 2);
    for b in data {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Hex-decode a string. Rejects odd lengths and non-hex characters instead
/// of slicing past the end (the panic behind #196).
pub(super) fn hex_decode(hex: &str) -> Result<Vec<u8>> {
    ensure!(
        hex.len().is_multiple_of(2),
        "odd-length hex ({} chars)",
        hex.len()
    );
    (0..hex.len())
        .step_by(2)
        .map(|i| {
            hex.get(i..i + 2)
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                .ok_or_else(|| anyhow::anyhow!("invalid hex at offset {i}"))
        })
        .collect()
}

/// XOR `data` with a repeating `key` (legacy format, read only for migration).
pub(super) fn xor_bytes(data: &[u8], key: &[u8]) -> Vec<u8> {
    if key.is_empty() {
        return data.to_vec();
    }
    data.iter()
        .enumerate()
        .map(|(i, b)| b ^ key[i % key.len()])
        .collect()
}

fn cipher(key: &[u8]) -> Result<Aes256Gcm> {
    ensure!(
        key.len() == KEY_LEN,
        "master key is {} bytes, expected {KEY_LEN}",
        key.len()
    );
    Ok(Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key)))
}

/// Encrypt plaintext with AES-256-GCM. Returns `"nonce_hex:ciphertext_hex"`.
pub(super) fn aes_encrypt(plaintext: &[u8], key: &[u8]) -> Result<String> {
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher(key)?
        .encrypt(&nonce, plaintext)
        .map_err(|e| anyhow::anyhow!("AES encrypt failed: {e}"))?;
    Ok(format!(
        "{}:{}",
        hex_encode(&nonce),
        hex_encode(&ciphertext)
    ))
}

/// Decrypt `"nonce_hex:ciphertext_hex"` with AES-256-GCM.
pub(super) fn aes_decrypt(encoded: &str, key: &[u8]) -> Result<Vec<u8>> {
    let (nonce_hex, ct_hex) = encoded
        .split_once(':')
        .ok_or_else(|| anyhow::anyhow!("missing nonce:ciphertext separator"))?;
    let nonce_bytes = hex_decode(nonce_hex).context("bad nonce")?;
    ensure!(
        nonce_bytes.len() == NONCE_LEN,
        "nonce is {} bytes, expected {NONCE_LEN}",
        nonce_bytes.len()
    );
    let ciphertext = hex_decode(ct_hex).context("bad ciphertext")?;
    cipher(key)?
        .decrypt(Nonce::from_slice(&nonce_bytes), ciphertext.as_ref())
        .map_err(|_| anyhow::anyhow!("authentication failed"))
}

/// A stored value, classified by its shape alone. The format decides the
/// decryption path, so a failed AES decrypt can never fall through to the
/// legacy XOR path, which "succeeds" with any key.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum StoredFormat {
    /// `nonce_hex:ciphertext_hex`, written by every orca since the AES switch.
    Aes,
    /// Plain even-length hex, from before the AES switch.
    LegacyXor,
    /// Neither: a corrupted or foreign value.
    Unknown,
}

pub(super) fn classify(stored: &str) -> StoredFormat {
    if stored.contains(':') {
        StoredFormat::Aes
    } else if hex_decode(stored).is_ok() {
        StoredFormat::LegacyXor
    } else {
        StoredFormat::Unknown
    }
}

/// Read and validate an existing master key.
pub(super) fn read_key(path: &Path) -> Result<Vec<u8>> {
    let key = std::fs::read(path)
        .with_context(|| format!("failed to read master key {}", path.display()))?;
    if key.len() != KEY_LEN {
        bail!(
            "master key {} is {} bytes, expected {KEY_LEN} (truncated or corrupted); \
             restore it from your backup",
            path.display(),
            key.len()
        );
    }
    Ok(key)
}

/// Create a new random master key at `path`, owner-only and all-or-nothing.
/// If another process created one concurrently, that key wins and is
/// returned, so both processes use the same key.
pub(super) fn create_key(path: &Path) -> Result<Vec<u8>> {
    let mut key = vec![0u8; KEY_LEN];
    {
        use std::io::Read;
        std::fs::File::open("/dev/urandom")
            .context("failed to open /dev/urandom")?
            .read_exact(&mut key)
            .context("failed to read random bytes")?;
    }
    if crate::fsutil::create_private_new(path, &key)
        .with_context(|| format!("failed to create master key {}", path.display()))?
    {
        Ok(key)
    } else {
        read_key(path)
    }
}

#[cfg(test)]
#[path = "crypto_tests.rs"]
mod tests;
