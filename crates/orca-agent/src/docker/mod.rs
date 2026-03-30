//! Docker container runtime implementing the [`Runtime`] trait via bollard.

mod config_builder;
mod runtime;
mod runtime_impl;
mod stats;

pub use runtime::ContainerRuntime;

/// Label applied to all orca-managed containers for identification and cleanup.
pub(crate) const ORCA_LABEL: &str = "orca.managed";
