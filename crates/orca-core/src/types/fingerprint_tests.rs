use crate::testing::spec;
use crate::types::{ResourceLimits, WorkloadSpec};

/// A named mutation applied to a test spec.
type Change = (&'static str, Box<dyn Fn(&mut WorkloadSpec)>);

#[test]
fn identical_specs_share_a_fingerprint() {
    assert_eq!(
        spec("web").compute_fingerprint(),
        spec("web").compute_fingerprint()
    );
}

#[test]
fn env_insertion_order_does_not_change_the_fingerprint() {
    // HashMap iteration order differs between maps built in different
    // orders (and between processes). Without sorting keys, master and
    // agent would disagree on an unchanged spec and churn the container.
    let keys: Vec<String> = (0..64).map(|i| format!("KEY_{i:02}")).collect();
    let mut a = spec("web");
    let mut b = spec("web");
    for k in &keys {
        a.env.insert(k.clone(), format!("v-{k}"));
    }
    for k in keys.iter().rev() {
        b.env.insert(k.clone(), format!("v-{k}"));
    }
    assert_eq!(a.compute_fingerprint(), b.compute_fingerprint());
}

#[test]
fn any_container_affecting_change_changes_the_fingerprint() {
    let base = spec("web").compute_fingerprint();
    let changes: Vec<Change> = vec![
        (
            "env",
            Box::new(|s| {
                s.env.insert("SMTP_HOST".into(), "smtp.example.com".into());
            }),
        ),
        ("image", Box::new(|s| s.image = "alpine:3.20".into())),
        ("cmd", Box::new(|s| s.cmd = vec!["/cron.sh".into()])),
        ("mounts", Box::new(|s| s.mounts = vec!["/a:/b:ro".into()])),
        (
            "resources",
            Box::new(|s| {
                s.resources = Some(ResourceLimits {
                    memory: Some("1Gi".into()),
                    cpu: Some(2.0),
                    gpu: None,
                })
            }),
        ),
    ];
    for (what, change) in changes {
        let mut s = spec("web");
        change(&mut s);
        assert_ne!(s.compute_fingerprint(), base, "{what} change not detected");
    }
}

#[test]
fn the_stamp_itself_is_excluded() {
    let mut s = spec("web");
    let before = s.compute_fingerprint();
    s.stamp_fingerprint();
    assert_eq!(s.fingerprint.as_deref(), Some(before.as_str()));
    // Re-computing on a stamped spec yields the same value, so an agent
    // comparing a received (stamped) spec never sees a false drift.
    assert_eq!(s.compute_fingerprint(), before);
}

#[test]
fn fingerprint_is_32_hex_chars() {
    let f = spec("web").compute_fingerprint();
    assert_eq!(f.len(), 32);
    assert!(f.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn old_masters_spec_without_the_field_still_deserializes() {
    let mut v = serde_json::to_value(spec("web")).unwrap();
    v.as_object_mut().unwrap().remove("fingerprint");
    let parsed: WorkloadSpec = serde_json::from_value(v).unwrap();
    assert!(parsed.fingerprint.is_none());
}
