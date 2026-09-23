//! Operation records and failure-injection modes for [`super::MockRuntime`].

use std::time::Duration;

/// The kind of a recorded operation.
///
/// Used both for counting recorded operations and for selecting which
/// operation an injected failure applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MockOpKind {
    /// [`crate::runtime::Runtime::create`]
    Create,
    /// [`crate::runtime::Runtime::start`]
    Start,
    /// [`crate::runtime::Runtime::stop`]
    Stop,
    /// [`crate::runtime::Runtime::remove`]
    Remove,
}

impl MockOpKind {
    /// Lower-case name, used in injected error messages.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Remove => "remove",
        }
    }
}

/// A single operation performed on the mock runtime, in call order.
///
/// Only *successful* operations are recorded. An injected failure returns an
/// error without appending a record, so the op log reads as "what actually
/// happened to the workload".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MockOp {
    /// A workload was created.
    Create(String),
    /// A workload was started.
    Start(String),
    /// A workload was stopped, with the grace period the caller requested.
    ///
    /// The timeout is recorded because the difference between a graceful stop
    /// and a force-remove is exactly what several regression tests assert: a
    /// `Stop` carrying a real grace period must precede `Remove`, rather than
    /// the container being torn down without one.
    Stop {
        /// Container name as the runtime saw it.
        name: String,
        /// Grace period passed by the caller.
        timeout: Duration,
    },
    /// A workload was removed.
    Remove(String),
}

impl MockOp {
    /// Which kind of operation this record is.
    pub fn kind(&self) -> MockOpKind {
        match self {
            Self::Create(_) => MockOpKind::Create,
            Self::Start(_) => MockOpKind::Start,
            Self::Stop { .. } => MockOpKind::Stop,
            Self::Remove(_) => MockOpKind::Remove,
        }
    }

    /// The workload name this record refers to.
    ///
    /// `create` is called with a [`crate::types::WorkloadSpec`] (bare name)
    /// while the other methods take a [`crate::runtime::WorkloadHandle`]
    /// (prefixed `orca-<name>`). The prefix is stripped here so records from
    /// both sources correlate to one workload.
    pub fn workload(&self) -> &str {
        let raw = match self {
            Self::Create(n) | Self::Start(n) | Self::Remove(n) => n,
            Self::Stop { name, .. } => name,
        };
        raw.strip_prefix("orca-").unwrap_or(raw)
    }

    /// The grace period, for a [`MockOp::Stop`] record only.
    pub fn stop_timeout(&self) -> Option<Duration> {
        match self {
            Self::Stop { timeout, .. } => Some(*timeout),
            _ => None,
        }
    }
}

/// How many times an injected failure should fire before clearing itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FailMode {
    /// Fail this many more calls, then stop failing.
    Times(usize),
    /// Fail every call until explicitly cleared.
    Always,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workload_strips_the_container_prefix() {
        assert_eq!(MockOp::Create("web".into()).workload(), "web");
        assert_eq!(MockOp::Start("orca-web".into()).workload(), "web");
        assert_eq!(MockOp::Remove("orca-web".into()).workload(), "web");
        let stop = MockOp::Stop {
            name: "orca-web".into(),
            timeout: Duration::from_secs(30),
        };
        assert_eq!(stop.workload(), "web");
    }

    #[test]
    fn create_and_stop_records_correlate_to_one_workload() {
        // The regression tests filter by workload across both sources.
        let create = MockOp::Create("db".into());
        let stop = MockOp::Stop {
            name: "orca-db".into(),
            timeout: Duration::from_secs(10),
        };
        assert_eq!(create.workload(), stop.workload());
    }

    #[test]
    fn stop_timeout_is_only_set_for_stop() {
        let stop = MockOp::Stop {
            name: "orca-web".into(),
            timeout: Duration::from_secs(45),
        };
        assert_eq!(stop.stop_timeout(), Some(Duration::from_secs(45)));
        assert_eq!(MockOp::Remove("orca-web".into()).stop_timeout(), None);
    }

    #[test]
    fn kind_maps_each_variant() {
        assert_eq!(MockOp::Create("a".into()).kind(), MockOpKind::Create);
        assert_eq!(MockOp::Start("a".into()).kind(), MockOpKind::Start);
        assert_eq!(MockOp::Remove("a".into()).kind(), MockOpKind::Remove);
        assert_eq!(
            MockOp::Stop {
                name: "a".into(),
                timeout: Duration::ZERO
            }
            .kind(),
            MockOpKind::Stop
        );
    }
}
