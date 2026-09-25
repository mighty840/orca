//! #211: every backend's random host port was published on 0.0.0.0, although
//! only this node's proxy (over 127.0.0.1) ever connects to it.

use orca_core::testing::spec;

use super::{build_container_config, public_database_port};

/// `(host_ip, host_port)` bound for `container_port`.
fn binding(spec: &orca_core::types::WorkloadSpec, container_port: &str) -> (String, String) {
    let config = build_container_config(spec);
    let bindings = config.host_config.unwrap().port_bindings.unwrap();
    let b = &bindings[container_port].as_ref().unwrap()[0];
    (b.host_ip.clone().unwrap(), b.host_port.clone().unwrap())
}

#[test]
fn a_random_backend_port_is_published_on_loopback_only() {
    let mut s = spec("gitea");
    s.port = Some(3000);

    assert_eq!(binding(&s, "3000/tcp"), ("127.0.0.1".into(), "0".into()));
}

#[test]
fn an_explicit_host_port_stays_public() {
    // coturn: TURN clients connect to 3478 from anywhere.
    let mut s = spec("coturn");
    s.port = Some(3478);
    s.host_port = Some(3478);

    assert_eq!(binding(&s, "3478/tcp"), ("0.0.0.0".into(), "3478".into()));
}

#[test]
fn extra_ports_keep_their_address() {
    let mut s = spec("jitsi-jvb");
    s.extra_ports = vec!["10000:10000/udp".into(), "127.0.0.1:54321:5432".into()];

    assert_eq!(binding(&s, "10000/udp"), ("0.0.0.0".into(), "10000".into()));
    assert_eq!(
        binding(&s, "5432/tcp"),
        ("127.0.0.1".into(), "54321".into())
    );
}

#[test]
fn only_public_database_ports_are_flagged() {
    assert_eq!(public_database_port("0.0.0.0", "5432"), Some("PostgreSQL"));
    assert_eq!(public_database_port("127.0.0.1", "5432"), None);
    assert_eq!(public_database_port("0.0.0.0", "22"), None);
    assert_eq!(public_database_port("0.0.0.0", "10000"), None);
}
