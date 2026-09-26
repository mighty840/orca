//! Image-declared `VOLUME`s the service doesn't cover (#184).
//!
//! Docker gives every `VOLUME` of the image that no mount covers exactly a
//! fresh anonymous volume on each container create, and leaves the previous
//! one dangling. For a cache that's harmless. For data it means a redeploy
//! silently starts empty: `postgres:18` moved its `VOLUME` to
//! `/var/lib/postgresql`, so a tag bump with `volume.path =
//! "/var/lib/postgresql/data"` would start an empty database.

use orca_core::types::WorkloadSpec;

/// How an uncovered image volume relates to what the service declares.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Uncovered {
    /// A declared volume or mount lies inside it: the rest of that directory
    /// is recreated empty on every deploy (partial coverage).
    Partially { inside: String },
    /// Nothing covers it.
    Entirely,
}

/// The image's `VOLUME` paths that no volume or mount of `spec` covers
/// exactly, sorted. Docker mounts an anonymous volume at exactly each
/// uncovered path, even if a parent or a child directory is mounted.
pub(crate) fn uncovered(
    image_volumes: impl IntoIterator<Item = String>,
    spec: &WorkloadSpec,
) -> Vec<(String, Uncovered)> {
    let declared: Vec<String> = spec
        .volume
        .iter()
        .map(|v| normalize(&v.path))
        .chain(
            spec.mounts
                .iter()
                .filter_map(|m| m.split(':').nth(1))
                .map(normalize),
        )
        .collect();
    let mut out: Vec<(String, Uncovered)> = image_volumes
        .into_iter()
        .map(|v| normalize(&v))
        .filter(|v| !declared.contains(v))
        .map(|v| {
            let inside = declared
                .iter()
                .find(|d| d.starts_with(&format!("{v}/")))
                .cloned();
            let how = match inside {
                Some(inside) => Uncovered::Partially { inside },
                None => Uncovered::Entirely,
            };
            (v, how)
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn normalize(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".into()
    } else {
        trimmed.into()
    }
}

#[cfg(test)]
mod tests {
    use orca_core::testing::spec;
    use orca_core::types::VolumeSpec;

    use super::*;

    fn with_volume(path: &str) -> WorkloadSpec {
        let mut s = spec("db");
        s.volume = Some(VolumeSpec {
            path: path.into(),
            size: None,
        });
        s
    }

    /// The postgres:18 trap: the image moved its VOLUME up one level.
    #[test]
    fn a_declared_path_inside_the_image_volume_is_partial_coverage() {
        let s = with_volume("/var/lib/postgresql/data");
        assert_eq!(
            uncovered(["/var/lib/postgresql".to_string()], &s),
            [(
                "/var/lib/postgresql".to_string(),
                Uncovered::Partially {
                    inside: "/var/lib/postgresql/data".into()
                }
            )]
        );
    }

    #[test]
    fn an_exact_volume_or_mount_covers_it() {
        let mut s = with_volume("/var/lib/postgresql/data");
        s.mounts = vec!["/srv/html:/var/www/html:ro".into()];
        let image = [
            "/var/lib/postgresql/data/".to_string(),
            "/var/www/html".to_string(),
        ];
        assert!(uncovered(image, &s).is_empty());
    }

    #[test]
    fn an_unrelated_image_volume_is_entirely_uncovered() {
        let s = with_volume("/data");
        assert_eq!(
            uncovered(["/var/cache/nginx".to_string()], &s),
            [("/var/cache/nginx".to_string(), Uncovered::Entirely)]
        );
    }
}
