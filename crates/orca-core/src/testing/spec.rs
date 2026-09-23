//! Minimal [`WorkloadSpec`] construction for tests.
//!
//! [`WorkloadSpec`] has thirty fields and no `Default`, so building one inline
//! costs twenty-five lines of noise per test. [`spec`] fills every field with a
//! harmless value and returns it for the caller to adjust:
//!
//! ```
//! use orca_core::testing::spec;
//!
//! let mut s = spec("web");
//! s.port = Some(8080);
//! ```
//!
//! Deliberately not an `impl Default for WorkloadSpec`: a spec with an empty
//! image is meaningless in production code, and this default belongs to tests.

use std::collections::HashMap;

use crate::types::{PullPolicy, Replicas, RuntimeKind, WorkloadSpec};

/// A minimal container spec named `name`, running `alpine:latest`, one replica.
pub fn spec(name: &str) -> WorkloadSpec {
    WorkloadSpec {
        name: name.to_string(),
        runtime: RuntimeKind::Container,
        image: "alpine:latest".to_string(),
        replicas: Replicas::Fixed(1),
        port: None,
        host_port: None,
        domain: None,
        domains: Vec::new(),
        routes: Vec::new(),
        health: None,
        readiness: None,
        liveness: None,
        env: HashMap::new(),
        resources: None,
        volume: None,
        deploy: None,
        placement: None,
        network: None,
        aliases: Vec::new(),
        mounts: Vec::new(),
        triggers: Vec::new(),
        build: None,
        tls_cert: None,
        tls_key: None,
        internal: false,
        cmd: Vec::new(),
        extra_ports: Vec::new(),
        strip_prefix: None,
        pull_policy: PullPolicy::default(),
        restart_policy: None,
        fingerprint: None,
    }
}

/// A minimal spec for `image`, so a test can stage a pull or tag change.
pub fn spec_with_image(name: &str, image: &str) -> WorkloadSpec {
    WorkloadSpec {
        image: image.to_string(),
        ..spec(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_is_a_single_container_replica() {
        let s = spec("web");
        assert_eq!(s.name, "web");
        assert_eq!(s.runtime, RuntimeKind::Container);
        assert!(matches!(s.replicas, Replicas::Fixed(1)));
    }

    #[test]
    fn spec_with_image_overrides_only_the_image() {
        let s = spec_with_image("db", "postgres:17-alpine");
        assert_eq!(s.image, "postgres:17-alpine");
        assert_eq!(s.name, "db");
    }
}
