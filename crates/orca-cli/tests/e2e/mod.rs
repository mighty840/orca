//! E2E test harness for the orca server.
//!
//! Provides [`OrcaServer`], a guard struct that spawns `orca server` as a child
//! process on a random port and kills it on drop.

use std::io::Write;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::time::Duration;

/// Find a free TCP port by binding to port 0.
pub fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("failed to bind ephemeral port");
    listener.local_addr().unwrap().port()
}

/// Guard struct that manages an `orca server` child process.
///
/// On [`Drop`], the process is killed and the temporary config file is cleaned up.
pub struct OrcaServer {
    child: Child,
    _config_path: PathBuf,
    pub api_port: u16,
    pub api_url: String,
}

impl OrcaServer {
    /// Start an orca server on random ports.
    ///
    /// Writes a minimal `cluster.toml` to a temp file and spawns the server
    /// binary. Waits up to 15 seconds for the health endpoint to respond.
    ///
    /// # Panics
    ///
    /// Panics if the binary cannot be found, the server fails to start, or
    /// the health endpoint does not become available within the timeout.
    pub async fn start() -> Self {
        let api_port = free_port();
        let proxy_port = free_port();

        // Write a temporary cluster.toml
        let config_dir = std::env::temp_dir().join(format!("orca-e2e-{api_port}"));
        std::fs::create_dir_all(&config_dir).expect("failed to create temp dir");
        let config_path = config_dir.join("cluster.toml");

        let config_content = format!(
            r#"[cluster]
name = "e2e-test"
api_port = {api_port}
"#
        );
        let mut f = std::fs::File::create(&config_path).expect("failed to create config");
        f.write_all(config_content.as_bytes())
            .expect("failed to write config");

        // Locate the orca binary — try cargo-built path first.
        let binary = Self::find_binary();

        let child = Command::new(&binary)
            .arg("server")
            .arg("--config")
            .arg(config_path.to_str().unwrap())
            .arg("--proxy-port")
            .arg(proxy_port.to_string())
            .env("RUST_LOG", "info")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("failed to spawn orca binary at {}: {e}", binary.display()));

        let api_url = format!("http://127.0.0.1:{api_port}");

        let mut server = Self {
            child,
            _config_path: config_path,
            api_port,
            api_url: api_url.clone(),
        };

        // Wait for the health endpoint to respond.
        let client = reqwest::Client::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        loop {
            if tokio::time::Instant::now() > deadline {
                server.dump_stderr();
                panic!("orca server did not become healthy within 15 seconds");
            }
            match client.get(format!("{api_url}/api/v1/health")).send().await {
                Ok(resp) if resp.status().is_success() => break,
                _ => tokio::time::sleep(Duration::from_millis(200)).await,
            }
        }

        server
    }

    /// Attempt to locate the orca binary in common build output directories.
    fn find_binary() -> PathBuf {
        // Walk up from CARGO_MANIFEST_DIR to find the workspace target dir.
        let manifest_dir =
            PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into()));
        let workspace_root = manifest_dir
            .parent()
            .and_then(|p| p.parent())
            .unwrap_or(&manifest_dir);

        let candidates = [
            workspace_root.join("target/debug/orca"),
            workspace_root.join("target/release/orca"),
        ];

        for c in &candidates {
            if c.exists() {
                return c.clone();
            }
        }

        panic!(
            "orca binary not found. Run `cargo build` first. Searched: {:?}",
            candidates
        );
    }

    /// Print captured stderr for debugging failed starts.
    fn dump_stderr(&mut self) {
        if let Some(stderr) = self.child.stderr.take() {
            let output = std::io::read_to_string(stderr).unwrap_or_default();
            eprintln!("=== orca server stderr ===\n{output}\n=== end ===");
        }
    }

    /// Return a [`reqwest::Client`] for making API calls.
    pub fn client(&self) -> reqwest::Client {
        reqwest::Client::new()
    }
}

impl Drop for OrcaServer {
    fn drop(&mut self) {
        // Kill the server process.
        let _ = self.child.kill();
        let _ = self.child.wait();

        // Clean up the temp config directory.
        if let Some(dir) = self._config_path.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// Skip the test unless `ORCA_E2E` env var is set.
///
/// Call at the top of each E2E test function.
pub fn require_e2e_env() {
    if std::env::var("ORCA_E2E").is_err() {
        eprintln!("Skipping E2E test: set ORCA_E2E=1 to run");
        return;
    }
}
