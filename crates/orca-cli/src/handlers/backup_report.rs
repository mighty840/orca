//! The outcome of one `orca backup` run (#197).
//!
//! `orca backup all` used to exit 0 whatever failed, and its last stdout
//! line, which the scheduler records and the dashboard shows, was the
//! config-file count ("Backup complete: 1 file(s)." after a night of volume
//! tarballs). Every part of the run now reports here. The summary is printed
//! last, and any failure makes the process exit non-zero.

/// What happened, part by part.
#[derive(Debug, Default)]
pub(crate) struct BackupReport {
    failures: Vec<String>,
    pub volumes_ok: u32,
    pub volumes_total: u32,
    /// Volume tarballs were age-encrypted (#231).
    pub volumes_encrypted: bool,
    pub hooks_run: u32,
    pub s3_ok: u32,
    pub s3_total: u32,
    /// The bind-mount summary (already counts archived / skipped / gaps).
    pub bind_mounts: Option<String>,
    pub config_stored: u32,
    pub config_skipped: u32,
    /// What retention did (or why it didn't run), #204.
    pub retention: Option<String>,
}

impl BackupReport {
    /// Record a failure. Printed on stderr right away, so it is visible in the
    /// journal even if the run dies later.
    pub(crate) fn fail(&mut self, what: impl Into<String>) {
        let what = what.into();
        eprintln!("BACKUP ERROR: {what}");
        self.failures.push(what);
    }

    pub(crate) fn ok(&self) -> bool {
        self.failures.is_empty()
    }

    /// One line covering the whole run. Printed last, so it is what the
    /// scheduler and agents record as the run's message.
    pub(crate) fn summary(&self) -> String {
        let mut parts = Vec::new();
        if self.volumes_total > 0 {
            let encrypted = if self.volumes_encrypted {
                " encrypted"
            } else {
                ""
            };
            parts.push(format!(
                "volumes {}/{}{encrypted} ({} pre-hook(s) run)",
                self.volumes_ok, self.volumes_total, self.hooks_run
            ));
        }
        if self.s3_total > 0 {
            parts.push(format!("S3 uploads {}/{}", self.s3_ok, self.s3_total));
        }
        if let Some(b) = &self.bind_mounts {
            parts.push(b.trim_start_matches("WARNING: ").to_string());
        }
        if self.config_stored + self.config_skipped > 0 {
            parts.push(format!(
                "config files {} stored, {} skipped",
                self.config_stored, self.config_skipped
            ));
        }
        if let Some(r) = &self.retention {
            parts.push(r.clone());
        }
        let detail = parts.join("; ");
        if self.ok() {
            format!("Backup OK: {detail}")
        } else {
            format!(
                "Backup FAILED ({}): {}. {detail}",
                self.failures.len(),
                self.failures.join("; ")
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clean_run_is_ok_and_says_what_it_covered() {
        let r = BackupReport {
            volumes_ok: 31,
            volumes_total: 31,
            hooks_run: 8,
            s3_ok: 32,
            s3_total: 32,
            config_stored: 5,
            ..Default::default()
        };
        assert!(r.ok());
        assert_eq!(
            r.summary(),
            "Backup OK: volumes 31/31 (8 pre-hook(s) run); S3 uploads 32/32; \
             config files 5 stored, 0 skipped"
        );
    }

    #[test]
    fn any_failure_makes_the_summary_lead_with_failed() {
        // The motivating case: S3 credentials rotated, every upload fails,
        // and the old summary still said "Backup complete: 1 file(s).".
        let mut r = BackupReport {
            volumes_ok: 31,
            volumes_total: 31,
            s3_total: 32,
            ..Default::default()
        };
        r.fail("S3 upload of orca-gitea-db-data failed: 403 Forbidden");
        assert!(!r.ok());
        let s = r.summary();
        assert!(
            s.starts_with("Backup FAILED (1): S3 upload of orca-gitea-db-data"),
            "{s}"
        );
        assert!(s.contains("S3 uploads 0/32"), "{s}");
    }
}
