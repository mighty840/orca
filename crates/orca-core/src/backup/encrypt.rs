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
    let out = tempfile::NamedTempFile::new().context("create temp file for encryption")?;
    encrypt_into(src, out.reopen().context("reopen temp file")?, recipients)?;
    Ok(out)
}

/// Encrypt `src` into `dst`, streaming, so multi-GB volume tarballs never sit
/// in memory. The ciphertext is written to an owner-only temporary file next
/// to `dst` and renamed into place, so `dst` never holds a partial file.
pub fn encrypt_file(src: &Path, dst: &Path, recipients: &[String]) -> Result<()> {
    let tmp = sibling_temp(dst)?;
    encrypt_into(src, tmp.reopen().context("reopen temp file")?, recipients)?;
    tmp.persist(dst)
        .with_context(|| format!("move ciphertext to {}", dst.display()))?;
    Ok(())
}

fn encrypt_into(src: &Path, out: std::fs::File, recipients: &[String]) -> Result<()> {
    let parsed = parse_recipients(recipients)?;
    let encryptor =
        age::Encryptor::with_recipients(parsed.iter().map(|r| r as &dyn age::Recipient))
            .context("age: cannot build encryptor")?;

    let mut input = BufReader::new(
        std::fs::File::open(src).with_context(|| format!("open {}", src.display()))?,
    );
    let mut writer = encryptor
        .wrap_output(std::io::BufWriter::new(out))
        .context("age: start stream")?;
    std::io::copy(&mut input, &mut writer).with_context(|| format!("encrypt {}", src.display()))?;
    writer.finish().context("age: finish stream")?.flush()?;
    Ok(())
}

/// An owner-only temporary file in `dst`'s directory, for an atomic rename.
fn sibling_temp(dst: &Path) -> Result<tempfile::NamedTempFile> {
    let dir = match dst.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    tempfile::NamedTempFile::new_in(dir)
        .with_context(|| format!("create temp file in {}", dir.display()))
}

/// Decrypt an age file with an `AGE-SECRET-KEY-…` identity into memory. For
/// config artifacts; use [`decrypt_to_file`] for anything large.
pub fn decrypt_file(src: &Path, identity: &str) -> Result<Vec<u8>> {
    let mut plain = Vec::new();
    decrypting_reader(src, identity)?.read_to_end(&mut plain)?;
    Ok(plain)
}

/// Decrypt `src` into `dst`, streaming, via an owner-only temporary file
/// renamed into place (a volume tarball can be several GB).
pub fn decrypt_to_file(src: &Path, identity: &str, dst: &Path) -> Result<()> {
    let mut reader = decrypting_reader(src, identity)?;
    let tmp = sibling_temp(dst)?;
    let mut out = std::io::BufWriter::new(tmp.reopen().context("reopen temp file")?);
    std::io::copy(&mut reader, &mut out).with_context(|| format!("decrypt {}", src.display()))?;
    out.flush()?;
    drop(out);
    tmp.persist(dst)
        .with_context(|| format!("move plaintext to {}", dst.display()))?;
    Ok(())
}

fn decrypting_reader(src: &Path, identity: &str) -> Result<impl Read> {
    let identity = age::x25519::Identity::from_str(identity.trim())
        .map_err(|e| anyhow::anyhow!("invalid age identity: {e}"))?;
    let file = std::fs::File::open(src).with_context(|| format!("open {}", src.display()))?;
    let decryptor = age::Decryptor::new(BufReader::new(file)).context("age: not an age file")?;
    decryptor
        .decrypt(std::iter::once(&identity as &dyn age::Identity))
        .context("age: cannot decrypt with this identity")
}

#[cfg(test)]
#[path = "encrypt_tests.rs"]
mod tests;
