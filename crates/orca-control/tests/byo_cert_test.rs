//! Unit tests for BYO TLS certificate loading.

use orca_control::reconciler::load_byo_cert;

#[test]
fn test_load_byo_cert_valid() {
    let dir = tempfile::tempdir().unwrap();
    let c = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    std::fs::write(dir.path().join("cert.pem"), c.cert.pem()).unwrap();
    std::fs::write(dir.path().join("key.pem"), c.key_pair.serialize_pem()).unwrap();
    let cp = dir.path().join("cert.pem");
    let kp = dir.path().join("key.pem");
    let ck = load_byo_cert(cp.to_str().unwrap(), kp.to_str().unwrap()).unwrap();
    assert!(
        !ck.cert.is_empty(),
        "should contain at least one certificate"
    );
}

#[test]
fn test_load_byo_cert_missing_file() {
    let result = load_byo_cert("/nonexistent/cert.pem", "/nonexistent/key.pem");
    assert!(result.is_err(), "should error on missing files");
}

#[test]
fn test_load_byo_cert_invalid_pem() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("cert.pem"), b"not a real cert").unwrap();
    std::fs::write(dir.path().join("key.pem"), b"not a real key").unwrap();
    let cp = dir.path().join("cert.pem");
    let kp = dir.path().join("key.pem");
    let result = load_byo_cert(cp.to_str().unwrap(), kp.to_str().unwrap());
    assert!(result.is_err(), "should error on garbage PEM content");
}
