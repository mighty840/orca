//! Client-side age encryption of backup artifacts (#117, #199).
//!
//! With `[backup] age_recipients` set, every config artifact is encrypted to
//! those public keys before it's written locally or uploaded, so read access
//! to the backup bucket no longer exposes `cluster.toml`'s tokens and S3 keys,
//! the webhook HMAC secrets, TLS private keys, or `master.key`. The matching
//! private key is kept out of band (a password manager), never on the cluster.
//!
//! Decrypt a downloaded artifact with the stock CLI:
//! `age -d -i key.txt secrets_20260923T030000Z.json.age > secrets.json`.

use std::io::{BufReader, Read, Write};
use std::path::Path;
use std::str::FromStr;

use anyhow::{Context, Result, bail};

/// Suffix appended to encrypted artifact names.
pub const AGE_SUFFIX: &str = ".age";

/// Parse `age1…` recipients, rejecting the whole list if any entry is bad, so
/// a typo can't silently reduce who can decrypt.
pub fn parse_recipients(recipients: &[String]) -> Result<Vec<age::x25519::Recipient>> {
    if recipients.is_empty() {
        bail!("no age recipients configured");
    }
    recipients
        .iter()
        .map(|r| {
            age::x25519::Recipient::from_str(r.trim())
                .map_err(|e| anyhow::anyhow!("invalid age recipient {r:?}: {e}"))
        })
        .collect()
}

/// Encrypt `src` to `recipients` into a new owner-only temporary file, which
/// is deleted when the returned handle drops. Any failure is an error: there
/// is no plaintext fallback.
pub fn encrypt_to_temp(src: &Path, recipients: &[String]) -> Result<tempfile::NamedTempFile> {
    let parsed = parse_recipients(recipients)?;
    let encryptor =
        age::Encryptor::with_recipients(parsed.iter().map(|r| r as &dyn age::Recipient))
            .context("age: cannot build encryptor")?;

    let mut input = BufReader::new(
        std::fs::File::open(src).with_context(|| format!("open {}", src.display()))?,
    );
    let out = tempfile::NamedTempFile::new().context("create temp file for encryption")?;
    let mut writer = encryptor
        .wrap_output(out.reopen().context("reopen temp file")?)
        .context("age: start stream")?;
    std::io::copy(&mut input, &mut writer).with_context(|| format!("encrypt {}", src.display()))?;
    writer.finish().context("age: finish stream")?.flush()?;
    Ok(out)
}

/// Decrypt an age file with an `AGE-SECRET-KEY-…` identity. Used by tests and
/// by restore once it learns about encrypted artifacts (#200).
pub fn decrypt_file(src: &Path, identity: &str) -> Result<Vec<u8>> {
    let identity = age::x25519::Identity::from_str(identity.trim())
        .map_err(|e| anyhow::anyhow!("invalid age identity: {e}"))?;
    let file = std::fs::File::open(src).with_context(|| format!("open {}", src.display()))?;
    let decryptor = age::Decryptor::new(BufReader::new(file)).context("age: not an age file")?;
    let mut reader = decryptor
        .decrypt(std::iter::once(&identity as &dyn age::Identity))
        .context("age: cannot decrypt with this identity")?;
    let mut plain = Vec::new();
    reader.read_to_end(&mut plain)?;
    Ok(plain)
}

#[cfg(test)]
#[path = "encrypt_tests.rs"]
mod tests;
