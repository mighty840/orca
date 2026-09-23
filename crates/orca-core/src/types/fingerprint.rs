//! Stable fingerprint of a declared [`WorkloadSpec`] (#213).
//!
//! The master stamps it when it turns a service config into a spec, before
//! any build step replaces the image with a freshly built tag. So the deploy
//! path and an agent's rejoin re-sync compute the same value for the same
//! config. The agent stores it as a container label and compares it on
//! rejoin, so a spec change that never reached a disconnected agent is
//! applied instead of being skipped because the old container is running.

use sha2::{Digest, Sha256};

use super::WorkloadSpec;

/// Container label that carries the fingerprint.
pub const FINGERPRINT_LABEL: &str = "orca.spec-fingerprint";

impl WorkloadSpec {
    /// Hex SHA-256 (first 16 bytes) over the spec's canonical JSON, with the
    /// `fingerprint` field itself left out. Object keys are sorted before
    /// hashing because `HashMap` fields (`env`) serialize in arbitrary order,
    /// and serde_json keeps insertion order when `preserve_order` is on.
    pub fn compute_fingerprint(&self) -> String {
        let mut unstamped = self.clone();
        unstamped.fingerprint = None;
        let value = serde_json::to_value(&unstamped).unwrap_or(serde_json::Value::Null);
        let mut canonical = String::new();
        write_canonical(&value, &mut canonical);
        let digest = Sha256::digest(canonical.as_bytes());
        digest[..16].iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Set `fingerprint` from the spec's current contents.
    pub fn stamp_fingerprint(&mut self) {
        self.fingerprint = Some(self.compute_fingerprint());
    }
}

/// Serialize `value` as JSON with every object's keys in sorted order.
fn write_canonical(value: &serde_json::Value, out: &mut String) {
    match value {
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::Value::String(key.clone()).to_string());
                out.push(':');
                write_canonical(&map[key], out);
            }
            out.push('}');
        }
        serde_json::Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

#[cfg(test)]
#[path = "fingerprint_tests.rs"]
mod tests;
