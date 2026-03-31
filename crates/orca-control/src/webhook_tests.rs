use super::*;

#[test]
fn branch_from_ref_extracts_name() {
    assert_eq!(branch_from_ref("refs/heads/main"), Some("main"));
    assert_eq!(branch_from_ref("refs/heads/feat/foo"), Some("feat/foo"));
    assert_eq!(branch_from_ref("refs/tags/v1.0"), None);
}

#[test]
fn validate_signature_works() {
    let secret = "mysecret";
    let body = b"hello world";
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(body);
    let result = mac.finalize().into_bytes();
    let sig = format!("sha256={}", hex::encode(result));
    assert!(validate_signature(secret, body, &sig));
    assert!(!validate_signature(secret, body, "sha256=badbeef"));
    assert!(!validate_signature(secret, body, "invalid"));
}
