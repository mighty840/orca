//! `${secrets.KEY}` substitution in env-var values.

use std::collections::{BTreeSet, HashMap};

use anyhow::{Result, bail};

use super::SecretStore;

const PREFIX: &str = "${secrets.";

impl SecretStore {
    /// Replace `${secrets.KEY}` patterns in env-var values with actual secret values.
    pub fn resolve_env(&self, env: &HashMap<String, String>) -> HashMap<String, String> {
        self.resolve_env_scoped(env, None)
    }

    /// Like [`Self::resolve_env`], but with project-first resolution (#68):
    /// for a service in project `p`, `${secrets.KEY}` resolves `p.KEY`
    /// before falling back to the bare `KEY`. Explicit cross-project refs
    /// (`${secrets.other.KEY}`) keep working — the prefixed form is looked
    /// up as written after the project-qualified attempt misses.
    ///
    /// Unknown references are left in place verbatim. Deploys use
    /// [`Self::resolve_env_checked`] instead, which refuses them.
    pub fn resolve_env_scoped(
        &self,
        env: &HashMap<String, String>,
        project: Option<&str>,
    ) -> HashMap<String, String> {
        let mut missing = BTreeSet::new();
        env.iter()
            .map(|(k, v)| (k.clone(), self.resolve_value(v, project, &mut missing)))
            .collect()
    }

    /// [`Self::resolve_env_scoped`], but an error naming every referenced
    /// secret that doesn't exist (#183). A container must never start with a
    /// literal `${secrets.X}` as its credential while the deploy reports success.
    pub fn resolve_env_checked(
        &self,
        env: &HashMap<String, String>,
        project: Option<&str>,
    ) -> Result<HashMap<String, String>> {
        let mut missing = BTreeSet::new();
        let resolved = env
            .iter()
            .map(|(k, v)| (k.clone(), self.resolve_value(v, project, &mut missing)))
            .collect();
        if !missing.is_empty() {
            let names: Vec<String> = missing.into_iter().collect();
            bail!(
                "unknown secret(s) referenced: {}. Create them with `orca secrets set` \
                 or remove the references",
                names.join(", ")
            );
        }
        Ok(resolved)
    }

    /// Look a reference key up project-first, then bare.
    fn lookup_scoped(&self, key: &str, project: Option<&str>) -> Option<&String> {
        if let Some(p) = project
            && let Some(v) = self.secrets.get(&format!("{p}.{key}"))
        {
            return Some(v);
        }
        self.secrets.get(key)
    }

    /// Substitute every `${secrets.KEY}` in `value`, left to right. A
    /// substituted secret value is never re-scanned, so a value that itself
    /// contains `${secrets.…}` is inserted as-is (and can't loop). Unknown keys
    /// stay verbatim and are added to `missing`.
    fn resolve_value(
        &self,
        value: &str,
        project: Option<&str>,
        missing: &mut BTreeSet<String>,
    ) -> String {
        let mut out = String::with_capacity(value.len());
        let mut rest = value;
        while let Some(start) = rest.find(PREFIX) {
            let after_prefix = start + PREFIX.len();
            let Some(end) = rest[after_prefix..].find('}') else {
                break;
            };
            let key = &rest[after_prefix..after_prefix + end];
            out.push_str(&rest[..start]);
            match self.lookup_scoped(key, project) {
                Some(secret) => out.push_str(secret),
                None => {
                    missing.insert(key.to_string());
                    out.push_str(&rest[start..after_prefix + end + 1]);
                }
            }
            rest = &rest[after_prefix + end + 1..];
        }
        out.push_str(rest);
        out
    }
}

#[cfg(test)]
#[path = "resolve_tests.rs"]
mod tests;
