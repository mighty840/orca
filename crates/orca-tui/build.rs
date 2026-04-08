fn main() {
    // Embed git commit hash in the binary so the TUI footer can show
    // exactly which build is talking to the master.
    let hash = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=ORCA_COMMIT={}", hash.trim());
}
