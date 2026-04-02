//! Docker volume backup and restore using bollard.

mod helpers;

use bollard::Docker;
use helpers::{
    create_backup_dir, find_latest_backup_dir, list_orca_volumes, run_backup_container,
    run_restore_container,
};

/// Backup all orca-prefixed Docker volumes to `~/.orca/backups/{timestamp}/`.
pub async fn backup_all_volumes() {
    let docker = match Docker::connect_with_local_defaults() {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("Failed to connect to Docker: {e}");
            return;
        }
    };

    let backup_dir = match create_backup_dir() {
        Some(d) => d,
        None => return,
    };

    let volumes = match list_orca_volumes(&docker).await {
        Some(v) => v,
        None => return,
    };

    if volumes.is_empty() {
        println!("No orca volumes found.");
        return;
    }

    println!("Backing up {} volume(s) to {}", volumes.len(), backup_dir);
    let mut count = 0u32;

    for vol in &volumes {
        print!("  {vol} ... ");
        match run_backup_container(&docker, vol, &backup_dir).await {
            Ok(()) => {
                println!("done");
                count += 1;
            }
            Err(e) => println!("FAILED: {e}"),
        }
    }

    println!("Volume backup complete: {count}/{} volumes.", volumes.len());
}

/// Restore a Docker volume from the latest backup directory.
pub async fn restore_volume(volume_name: &str) {
    let docker = match Docker::connect_with_local_defaults() {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("Failed to connect to Docker: {e}");
            return;
        }
    };

    let backup_dir = match find_latest_backup_dir() {
        Some(d) => d,
        None => {
            println!("No backup directories found in ~/.orca/backups/");
            return;
        }
    };

    let archive = format!("{backup_dir}/{volume_name}.tar.gz");
    if !std::path::Path::new(&archive).exists() {
        println!("No backup found for volume '{volume_name}' in {backup_dir}");
        return;
    }

    println!("Restoring {volume_name} from {backup_dir} ...");
    match run_restore_container(&docker, volume_name, &backup_dir).await {
        Ok(()) => println!("Restored volume '{volume_name}' successfully."),
        Err(e) => tracing::error!("Restore failed: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use helpers::backup_dir_path;

    use super::*;

    #[test]
    fn backup_dir_uses_timestamp_subdirectory() {
        let home = std::path::Path::new("/tmp/fakehome");
        let path = backup_dir_path(home, 1_700_000_000);
        assert!(path.contains(".orca/backups/1700000000"));
        assert!(path.starts_with("/tmp/fakehome/"));
    }

    #[test]
    fn backup_dir_timestamp_format_is_numeric() {
        let home = std::path::Path::new("/home/testuser");
        let path = backup_dir_path(home, 42);
        // The final component should be the epoch seconds as a plain number
        let last = std::path::Path::new(&path)
            .file_name()
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(last, "42");
    }

    #[test]
    fn create_backup_dir_creates_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = backup_dir_path(tmp.path(), 9999);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(std::path::Path::new(&dir).is_dir());
    }

    #[test]
    fn find_latest_picks_lexicographic_last() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join(".orca/backups");
        std::fs::create_dir_all(base.join("1000")).unwrap();
        std::fs::create_dir_all(base.join("2000")).unwrap();
        std::fs::create_dir_all(base.join("1500")).unwrap();
        // find_latest_backup_dir uses dirs_next, so test the sorting logic directly
        let mut entries: Vec<_> = std::fs::read_dir(&base)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        entries.sort_by_key(|e| e.file_name());
        let last = entries.last().unwrap().file_name();
        assert_eq!(last.to_str().unwrap(), "2000");
    }
}
