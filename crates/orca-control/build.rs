fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Use local copy for crates.io packaging, workspace path for development
    let proto = if std::path::Path::new("proto/orca.proto").exists() {
        "proto/orca.proto"
    } else {
        "../../proto/orca.proto"
    };
    tonic_build::compile_protos(proto)?;

    // Embed git commit so the cluster_info endpoint can report which build
    // the master is running.
    let hash = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_else(|| "unknown".into());
    println!("cargo:rustc-env=ORCA_COMMIT={}", hash.trim());
    Ok(())
}
