fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Use local copy for crates.io packaging, workspace path for development
    let proto = if std::path::Path::new("proto/orca.proto").exists() {
        "proto/orca.proto"
    } else {
        "../../proto/orca.proto"
    };
    tonic_build::compile_protos(proto)?;
    Ok(())
}
