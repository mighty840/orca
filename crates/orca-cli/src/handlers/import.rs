use crate::subcommands::ImportSource;

pub fn handle_import(source: ImportSource) {
    match source {
        ImportSource::DockerCompose { file, analyze } => {
            println!("Importing from docker-compose: {file}");
            let path = std::path::Path::new(&file);
            match orca_core::import::compose::parse_compose_file(path) {
                Ok(config) => {
                    println!("Found {} services.", config.service.len());
                    write_services_toml(&config);
                }
                Err(e) => tracing::error!("Failed to parse {file}: {e}"),
            }
            if analyze {
                println!("AI analysis not yet connected.");
            }
        }
        ImportSource::Coolify { path, analyze } => {
            println!("Importing from Coolify: {path}");
            let coolify_path = std::path::Path::new(&path);
            match orca_core::import::coolify::import_coolify_dir(coolify_path) {
                Ok(config) => {
                    println!("Found {} services.", config.service.len());
                    write_services_toml(&config);
                }
                Err(e) => tracing::error!("Failed to import from Coolify: {e}"),
            }
            if analyze {
                println!("AI analysis not yet connected.");
            }
        }
    }
}

fn write_services_toml(config: &orca_core::config::ServicesConfig) {
    match orca_core::import::compose::services_to_toml(config) {
        Ok(toml_str) => {
            if let Err(e) = std::fs::write("services.toml", &toml_str) {
                tracing::error!("Failed to write services.toml: {e}");
            } else {
                println!("Wrote services.toml");
            }
        }
        Err(e) => tracing::error!("Failed to serialize config: {e}"),
    }
}
