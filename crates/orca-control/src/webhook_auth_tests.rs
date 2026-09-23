use hmac::Mac;
use proptest::prelude::*;

use super::*;

fn sign(secret: &str, body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

fn config(secret: Option<&str>) -> WebhookConfig {
    WebhookConfig {
        repo: "org/app".into(),
        service_name: "app".into(),
        branch: "main".into(),
        secret: secret.map(str::to_string),
        infra: false,
    }
}

// --- validate_signature ----------------------------------------------------

#[test]
fn validate_signature_accepts_a_correct_signature() {
    let body = b"hello world";
    assert!(validate_signature(
        "mysecret",
        body,
        &sign("mysecret", body)
    ));
}

#[test]
fn validate_signature_rejects_wrong_secret_body_or_format() {
    let body = b"hello world";
    let good = sign("mysecret", body);
    assert!(!validate_signature("other", body, &good));
    assert!(!validate_signature("mysecret", b"tampered", &good));
    assert!(!validate_signature("mysecret", body, "sha256=badbeef"));
    assert!(!validate_signature("mysecret", body, "invalid"));
    assert!(!validate_signature("mysecret", body, ""));
}

#[test]
fn validate_signature_rejects_bad_hex() {
    assert!(!validate_signature("secret", b"body", "sha256=zzzz"));
}

// --- effective_secret -------------------------------------------------------

#[test]
fn effective_secret_is_none_when_absent() {
    assert_eq!(config(None).effective_secret(), None);
}

#[test]
fn effective_secret_treats_empty_and_blank_as_absent() {
    // An HMAC keyed with "" is computable by anyone, so it authenticates
    // nothing and must not count as configured.
    assert_eq!(config(Some("")).effective_secret(), None);
    assert_eq!(config(Some("   ")).effective_secret(), None);
    assert_eq!(config(Some("\t\n")).effective_secret(), None);
}

#[test]
fn effective_secret_returns_a_real_secret() {
    assert_eq!(config(Some("s3cret")).effective_secret(), Some("s3cret"));
}

// --- short_sha --------------------------------------------------------------

#[test]
fn short_sha_truncates_ascii_to_eight() {
    assert_eq!(short_sha("abc123def456"), "abc123de");
}

#[test]
fn short_sha_keeps_short_ids_whole() {
    assert_eq!(short_sha("abc"), "abc");
    assert_eq!(short_sha(""), "");
    assert_eq!(short_sha("unknown"), "unknown");
}

#[test]
fn short_sha_does_not_panic_on_the_reported_payload() {
    // "€€€" is nine bytes; the old `&id[..8]` cut inside the third character
    // and panicked before any webhook lookup or signature check.
    assert_eq!(short_sha("€€€"), "€€€");
}

#[test]
fn short_sha_does_not_panic_when_byte_eight_is_mid_character() {
    // Seven ASCII bytes then a three-byte char spanning bytes 7..10.
    assert_eq!(short_sha("abcdefg€"), "abcdefg€");
    // Nine characters: the cut lands after the eighth, on a boundary.
    assert_eq!(short_sha("abcdefg€x"), "abcdefg€");
}

proptest! {
    /// No input can make `short_sha` panic, and it only ever returns a prefix
    /// of at most eight characters. `commit_id` is attacker-controlled.
    #[test]
    fn short_sha_is_a_bounded_prefix_for_any_input(s in any::<String>()) {
        let out = short_sha(&s);
        prop_assert!(s.starts_with(out));
        prop_assert!(out.chars().count() <= 8);
        if s.chars().count() <= 8 {
            prop_assert_eq!(out, s.as_str());
        } else {
            prop_assert_eq!(out.chars().count(), 8);
        }
    }

    /// A signature over one body never validates a different body.
    #[test]
    fn signature_does_not_validate_a_different_body(
        secret in "[a-zA-Z0-9]{1,32}",
        a in any::<Vec<u8>>(),
        b in any::<Vec<u8>>(),
    ) {
        prop_assume!(a != b);
        prop_assert!(!validate_signature(&secret, &b, &sign(&secret, &a)));
    }
}
