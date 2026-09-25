//! Replacing a container without losing the old one on failure (#174).
//!
//! The replacement is created under the same name, so the old container has
//! to make way first. `create()` removes it (gracefully since #172). If the
//! new one then fails to create or start (a host port already bound, a bad
//! bind mount, a create-time error), the service is left with nothing.
//! Here the old container is stopped and renamed aside instead. It is
//! removed only once the replacement runs, and otherwise renamed back and
//! restarted, keeping its container id, so the master's handle stays valid.

use bollard::container::{RemoveContainerOptions, RenameContainerOptions, StopContainerOptions};
use orca_core::error::Result;
use orca_core::runtime::{Runtime, WorkloadHandle};
use orca_core::types::WorkloadSpec;
use tracing::{info, warn};

use super::ContainerRuntime;
use super::runtime_impl::{REPLACE_GRACE_SECS, is_status};

/// The name the old container waits under while its replacement starts.
pub(crate) fn aside_name(name: &str) -> String {
    format!("{name}.replaced")
}

impl ContainerRuntime {
    /// Create and start `spec`'s container, keeping any existing one aside
    /// until the new one runs. On failure the old one is back and running.
    pub(super) async fn replace(&self, spec: &WorkloadSpec) -> Result<WorkloadHandle> {
        // Pull and network setup first: failing here touches nothing.
        let (name, config, network) = self.prepare(spec).await?;
        let aside = self.set_aside(&name).await;

        let attempt = match self.create_named(spec, &name, config, &network).await {
            Ok(handle) => match self.start(&handle).await {
                Ok(()) => Ok(handle),
                Err(e) => {
                    self.force_remove(&handle.runtime_id).await;
                    Err(e)
                }
            },
            Err(e) => Err(e),
        };

        match (&attempt, aside) {
            (Ok(_), Some(aside)) => self.force_remove(&aside).await,
            (Err(e), Some(aside)) => {
                warn!("replacing {name} failed ({e}); restoring the previous container");
                self.restore(&aside, &name).await;
            }
            (_, None) => {}
        }
        attempt
    }

    /// Stop `name` gracefully and rename it aside. Returns the aside name, or
    /// `None` if there was nothing to set aside.
    async fn set_aside(&self, name: &str) -> Option<String> {
        let aside = aside_name(name);
        let primary_exists = self.exists(name).await;
        // A crash mid-replace can leave an aside container behind. If the
        // primary is gone it may be the only copy: bring it back first.
        if self.exists(&aside).await {
            if primary_exists {
                self.force_remove(&aside).await;
            } else {
                warn!("found {aside} without {name}: restoring it before replacing");
                self.restore(&aside, name).await;
            }
        }
        if !self.exists(name).await {
            return None;
        }

        match self
            .docker
            .stop_container(
                name,
                Some(StopContainerOptions {
                    t: REPLACE_GRACE_SECS,
                }),
            )
            .await
        {
            Ok(()) => info!("Stopped {name} before replacing it"),
            Err(e) if is_status(&e, &[304, 404]) => {}
            Err(e) => warn!("could not stop {name} gracefully: {e}"),
        }
        match self
            .docker
            .rename_container(
                name,
                RenameContainerOptions {
                    name: aside.clone(),
                },
            )
            .await
        {
            Ok(()) => Some(aside),
            Err(e) => {
                // Can't keep it: fall back to removing it, as before #174.
                warn!("could not set {name} aside ({e}); removing it instead");
                self.force_remove(name).await;
                None
            }
        }
    }

    /// Rename `aside` back to `name` and start it.
    async fn restore(&self, aside: &str, name: &str) {
        if let Err(e) = self
            .docker
            .rename_container(
                aside,
                RenameContainerOptions {
                    name: name.to_string(),
                },
            )
            .await
        {
            warn!("could not rename {aside} back to {name}: {e}");
            return;
        }
        match self.docker.start_container::<String>(name, None).await {
            Ok(()) => info!("Restored {name}"),
            Err(e) if is_status(&e, &[304]) => info!("Restored {name}"),
            Err(e) => warn!("restored {name} but could not start it: {e}"),
        }
    }

    async fn exists(&self, name: &str) -> bool {
        self.docker.inspect_container(name, None).await.is_ok()
    }

    async fn force_remove(&self, id_or_name: &str) {
        if let Err(e) = self
            .docker
            .remove_container(
                id_or_name,
                Some(RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await
            && !is_status(&e, &[404])
        {
            warn!("could not remove {id_or_name}: {e}");
        }
    }
}
