//! Token management: show, create, list API tokens.

use crate::subcommands::TokenAction;

/// Handle `orca token` subcommands.
pub fn handle_token(action: TokenAction) {
    match action {
        TokenAction::Show => show_token(),
        TokenAction::Create { name, role } => create_token(&name, &role),
        TokenAction::List => list_tokens(),
    }
}

/// Show the current cluster token (legacy).
fn show_token() {
    let path = token_path();
    if path.exists() {
        if let Ok(token) = std::fs::read_to_string(&path) {
            println!("{}", token.trim());
        }
    } else {
        println!("No cluster token found. Start the server first.");
    }
}

/// Create a new named API token and append to cluster.toml.
fn create_token(name: &str, role: &str) {
    // Validate role
    if !matches!(role, "admin" | "deployer" | "viewer") {
        eprintln!("Invalid role '{role}'. Must be: admin, deployer, or viewer");
        std::process::exit(1);
    }

    // Generate token
    let token = format!("{:x}{:x}", rand::random::<u64>(), rand::random::<u64>());

    // Append to cluster.toml
    let cluster_path = std::path::Path::new("cluster.toml");
    if !cluster_path.exists() {
        eprintln!("No cluster.toml found in current directory");
        std::process::exit(1);
    }

    let entry = format!("\n[[token]]\nname = \"{name}\"\nvalue = \"{token}\"\nrole = \"{role}\"\n");

    if let Err(e) = std::fs::OpenOptions::new()
        .append(true)
        .open(cluster_path)
        .and_then(|mut f| {
            use std::io::Write;
            f.write_all(entry.as_bytes())
        })
    {
        eprintln!("Failed to write to cluster.toml: {e}");
        std::process::exit(1);
    }

    println!("Token created:");
    println!("  Name:  {name}");
    println!("  Role:  {role}");
    println!("  Token: {token}");
    println!("\nRestart orca server to apply.");
}

/// List all configured tokens from cluster.toml.
fn list_tokens() {
    let cluster_path = std::path::Path::new("cluster.toml");
    if !cluster_path.exists() {
        // Show legacy token if exists
        show_token();
        return;
    }

    match orca_core::config::ClusterConfig::load(cluster_path) {
        Ok(config) => {
            if config.token.is_empty() && config.api_tokens.is_empty() {
                println!("No tokens configured.");
                return;
            }
            if !config.api_tokens.is_empty() {
                println!("Legacy tokens (all admin):");
                for (i, t) in config.api_tokens.iter().enumerate() {
                    println!(
                        "  {}. {}...{}",
                        i + 1,
                        &t[..8.min(t.len())],
                        &t[t.len().saturating_sub(4)..]
                    );
                }
            }
            if !config.token.is_empty() {
                println!("Named tokens:");
                for t in &config.token {
                    println!(
                        "  {:<20} {:>10}   {}...{}",
                        t.name,
                        format!("{:?}", t.role).to_lowercase(),
                        &t.value[..8.min(t.value.len())],
                        &t.value[t.value.len().saturating_sub(4)..]
                    );
                }
            }
        }
        Err(e) => eprintln!("Failed to read cluster.toml: {e}"),
    }
}

fn token_path() -> std::path::PathBuf {
    std::env::var("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| ".".into())
        .join(".orca/cluster.token")
}
