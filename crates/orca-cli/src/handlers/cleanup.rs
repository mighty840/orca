//! Docker cleanup: remove stopped containers, dangling images, and unused volumes.

use std::collections::HashMap;

use anyhow::Result;
use bollard::Docker;
use bollard::container::{ListContainersOptions, RemoveContainerOptions};
use bollard::image::PruneImagesOptions;
use bollard::volume::PruneVolumesOptions;
use tracing::info;

/// Remove unused Docker resources and print a summary.
pub async fn handle_cleanup() -> Result<()> {
    let docker = Docker::connect_with_local_defaults()?;

    println!("Cleaning up unused Docker resources...\n");

    // Remove stopped containers (not orca-managed)
    let mut filters = HashMap::new();
    filters.insert("status", vec!["exited"]);
    let opts = ListContainersOptions {
        all: true,
        filters,
        ..Default::default()
    };
    let containers = docker.list_containers(Some(opts)).await?;

    let mut removed_containers = 0u32;
    for container in &containers {
        // Skip orca-managed containers
        let is_orca = container
            .labels
            .as_ref()
            .is_some_and(|l| l.contains_key("orca.managed"));
        if is_orca {
            continue;
        }
        let id = container.id.as_deref().unwrap_or_default();
        let rm_opts = RemoveContainerOptions {
            force: false,
            ..Default::default()
        };
        if let Err(e) = docker.remove_container(id, Some(rm_opts)).await {
            info!(container = %id, error = %e, "failed to remove container");
        } else {
            removed_containers += 1;
        }
    }

    // Prune dangling images
    let mut img_filters = HashMap::new();
    img_filters.insert("dangling", vec!["true"]);
    let prune_result = docker
        .prune_images(Some(PruneImagesOptions {
            filters: img_filters,
        }))
        .await?;
    let removed_images = prune_result.images_deleted.as_ref().map_or(0, |v| v.len());

    // Prune unused volumes
    let vol_result = docker
        .prune_volumes(None::<PruneVolumesOptions<String>>)
        .await?;
    let removed_volumes = vol_result.volumes_deleted.as_ref().map_or(0, |v| v.len());

    println!("Cleanup complete:");
    println!("  Containers removed: {removed_containers}");
    println!("  Dangling images removed: {removed_images}");
    println!("  Unused volumes removed: {removed_volumes}");

    Ok(())
}
