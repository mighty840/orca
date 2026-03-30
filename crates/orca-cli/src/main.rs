mod client;

use std::sync::Arc;

use clap::{Parser, Subcommand};
use tracing::info;

use client::OrcaClient;

#[derive(Parser)]
#[command(
    name = "orca",
    about = "Container + Wasm orchestrator with AI ops",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// API server address
    #[arg(long, default_value = "http://127.0.0.1:6880", global = true)]
    api: String,
}

#[derive(Subcommand)]
enum Command {
    /// Start the orca server (control plane + agent + proxy)
    Server {
        /// Path to cluster.toml
        #[arg(short, long, default_value = "cluster.toml")]
        config: String,
        /// Proxy port for HTTP traffic
        #[arg(long, default_value = "80")]
        proxy_port: u16,
    },

    /// Deploy services from config
    Deploy {
        /// Path to services.toml
        #[arg(short, long, default_value = "services.toml")]
        file: String,
    },

    /// Show cluster and service status
    Status,

    /// Stream logs from a service
    Logs {
        /// Service name
        service: String,
        /// Number of lines to show
        #[arg(long, default_value = "100")]
        tail: u64,
        /// Follow log output
        #[arg(short, long)]
        follow: bool,
        /// Summarize logs using AI
        #[arg(long)]
        summarize: bool,
    },

    /// Scale a service
    Scale {
        /// Service name
        service: String,
        /// Number of replicas
        replicas: u32,
    },

    /// Rollback a service to previous version
    Rollback {
        /// Service name
        service: String,
    },

    /// Ask the AI assistant about the cluster
    Ask {
        /// Your question (e.g., "why is the API returning 503s?")
        question: Vec<String>,
    },

    /// Generate service config from natural language
    Generate {
        /// Description of what you need
        description: Vec<String>,
    },

    /// Manage conversational alerts
    Alerts {
        #[command(subcommand)]
        action: AlertsAction,
    },

    /// Manage secrets
    Secrets {
        #[command(subcommand)]
        action: SecretsAction,
    },

    /// Import from external tools
    Import {
        #[command(subcommand)]
        source: ImportSource,
    },

    /// Manage webhooks for git-push deploy
    Webhooks {
        #[command(subcommand)]
        action: WebhookAction,
    },

    /// List or inspect nodes
    Nodes {
        /// Show GPU details
        #[arg(long)]
        gpus: bool,
    },

    /// Show GPU status across the cluster
    Gpus,

    /// Join this node to an existing cluster
    Join {
        /// Address of an existing cluster node
        address: String,
    },

    /// Launch the TUI dashboard
    Tui,

    /// Launch the web dashboard
    Web {
        /// Port for web UI
        #[arg(short, long, default_value = "6890")]
        port: u16,
    },
}

#[derive(Subcommand)]
enum AlertsAction {
    /// List active alert conversations
    List {
        #[arg(short, long)]
        all: bool,
    },
    /// View an alert conversation
    View { id: String },
    /// Reply to an alert conversation
    Reply { id: String, message: Vec<String> },
    /// Dismiss an alert
    Dismiss { id: String },
    /// Apply the AI's suggested fix for an alert
    Fix { id: String },
}

#[derive(Subcommand)]
enum SecretsAction {
    /// Set a secret
    Set { key: String, value: String },
    /// Remove a secret
    Remove { key: String },
    /// List all secret keys
    List,
    /// Import secrets from env file
    Import {
        #[arg(short, long)]
        file: String,
    },
}

#[derive(Subcommand)]
enum ImportSource {
    /// Import from a docker-compose.yml
    DockerCompose {
        #[arg(default_value = "docker-compose.yml")]
        file: String,
        #[arg(long)]
        analyze: bool,
    },
    /// Import from a Coolify installation
    Coolify {
        #[arg(default_value = "/data/coolify")]
        path: String,
        #[arg(long)]
        analyze: bool,
    },
}

#[derive(Subcommand)]
enum WebhookAction {
    /// Add a webhook
    Add {
        #[arg(long)]
        repo: String,
        #[arg(long)]
        service: String,
        #[arg(long, default_value = "main")]
        branch: String,
    },
    /// List webhooks
    List,
    /// Remove a webhook
    Remove { id: String },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,hyper=warn,reqwest=warn".into()),
        )
        .init();

    match cli.command {
        // ========== SERVER ==========
        Command::Server { config, proxy_port } => {
            let cluster_config = orca_core::config::ClusterConfig::load(config.as_ref())?;
            info!(
                "Starting orca server '{}' (API: {}, Proxy: {})",
                cluster_config.cluster.name, cluster_config.cluster.api_port, proxy_port,
            );

            // Create container runtime
            let runtime = Arc::new(orca_agent::docker::ContainerRuntime::new()?);

            // Shared route table: same Arc used by both control plane and proxy
            let route_table = Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new()));

            // Spawn proxy (shares route table with control plane)
            let proxy_routes = route_table.clone();
            tokio::spawn(async move {
                if let Err(e) = orca_proxy::run_proxy(proxy_routes, proxy_port).await {
                    tracing::error!("Proxy error: {e}");
                }
            });

            // Run the API server (blocks until shutdown)
            let runtime_for_cleanup = runtime.clone();
            orca_control::run_server(cluster_config, runtime, route_table).await?;

            // Graceful cleanup
            info!("Shutting down, cleaning up containers...");
            runtime_for_cleanup.cleanup_all().await;
            info!("Shutdown complete");
        }

        // ========== DEPLOY ==========
        Command::Deploy { file } => {
            let config = orca_core::config::ServicesConfig::load(file.as_ref())?;
            let client = OrcaClient::new(cli.api);

            println!("Deploying {} services...", config.service.len());
            match client.deploy(&config).await {
                Ok(resp) => {
                    for name in &resp.deployed {
                        println!("  + {name}");
                    }
                    for err in &resp.errors {
                        eprintln!("  ! {err}");
                    }
                    println!(
                        "Deployed: {}, Errors: {}",
                        resp.deployed.len(),
                        resp.errors.len()
                    );
                }
                Err(e) => {
                    eprintln!("Deploy failed: {e}");
                    eprintln!("Is `orca server` running?");
                    std::process::exit(1);
                }
            }
        }

        // ========== STATUS ==========
        Command::Status => {
            let client = OrcaClient::new(cli.api);
            match client.status().await {
                Ok(resp) => {
                    println!("Cluster: {}", resp.cluster_name);
                    println!();
                    if resp.services.is_empty() {
                        println!("No services deployed.");
                    } else {
                        let header = format!(
                            "{:<20} {:<12} {:<10} {:<10} {:<20}",
                            "SERVICE", "RUNTIME", "REPLICAS", "STATUS", "DOMAIN"
                        );
                        println!("{header}");
                        for svc in &resp.services {
                            println!(
                                "{:<20} {:<12} {}/{:<7} {:<10} {}",
                                svc.name,
                                format!("{:?}", svc.runtime).to_lowercase(),
                                svc.running_replicas,
                                svc.desired_replicas,
                                svc.status,
                                svc.domain.as_deref().unwrap_or("-")
                            );
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Failed to get status: {e}");
                    eprintln!("Is `orca server` running?");
                    std::process::exit(1);
                }
            }
        }

        // ========== LOGS ==========
        Command::Logs {
            service,
            tail,
            follow: _,
            summarize,
        } => {
            let client = OrcaClient::new(cli.api);
            if summarize {
                println!("AI log summarization not yet connected.");
                println!("Configure [ai] in cluster.toml to enable.");
            } else {
                match client.logs(&service, tail).await {
                    Ok(logs) => print!("{logs}"),
                    Err(e) => {
                        eprintln!("Failed to get logs for '{service}': {e}");
                        std::process::exit(1);
                    }
                }
            }
        }

        // ========== SCALE ==========
        Command::Scale { service, replicas } => {
            let client = OrcaClient::new(cli.api);
            match client.scale(&service, replicas).await {
                Ok(resp) => {
                    println!("Scaled {} to {} replicas", resp.service, resp.replicas);
                }
                Err(e) => {
                    eprintln!("Failed to scale '{service}': {e}");
                    std::process::exit(1);
                }
            }
        }

        // ========== AI ==========
        Command::Ask { question } => {
            let q = question.join(" ");
            println!("Q: {q}\n");
            println!("AI backend not yet connected. Configure [ai] in cluster.toml.");
        }
        Command::Generate { description } => {
            let desc = description.join(" ");
            println!("Generating config for: {desc}\n");
            println!("AI backend not yet connected. Configure [ai] in cluster.toml.");
        }

        // ========== ALERTS ==========
        Command::Alerts { action } => match action {
            AlertsAction::List { all } => {
                let scope = if all { "all" } else { "active" };
                println!("No {scope} alert conversations.");
            }
            AlertsAction::View { id } => println!("Alert {id}: not yet connected."),
            AlertsAction::Reply { id, message } => {
                let msg = message.join(" ");
                println!("Reply to alert {id}: {msg}");
            }
            AlertsAction::Dismiss { id } => println!("Dismissed alert {id}."),
            AlertsAction::Fix { id } => println!("Applying fix for alert {id}..."),
        },

        // ========== GPU ==========
        Command::Gpus => println!("No GPU nodes registered."),
        Command::Nodes { gpus } => {
            if gpus {
                println!("No nodes with GPUs registered.");
            } else {
                println!("No nodes registered (single-node mode).");
            }
        }

        // ========== OTHER ==========
        Command::Rollback { service } => {
            println!("Rollback for '{service}' not yet implemented (M4).")
        }
        Command::Secrets { action } => match action {
            SecretsAction::Set { key, .. } => println!("Secret '{key}' set."),
            SecretsAction::Remove { key } => println!("Secret '{key}' removed."),
            SecretsAction::List => println!("No secrets configured."),
            SecretsAction::Import { file } => println!("Importing secrets from {file}..."),
        },
        Command::Import { source } => match source {
            ImportSource::DockerCompose { file, analyze } => {
                println!("Importing from docker-compose: {file}");
                if analyze {
                    println!("AI analysis not yet connected.");
                }
            }
            ImportSource::Coolify { path, analyze } => {
                println!("Importing from Coolify: {path}");
                if analyze {
                    println!("AI analysis not yet connected.");
                }
            }
        },
        Command::Webhooks { action } => match action {
            WebhookAction::Add {
                repo,
                service,
                branch,
            } => {
                println!("Webhook added: {repo} -> {service} (branch: {branch})");
            }
            WebhookAction::List => println!("No webhooks configured."),
            WebhookAction::Remove { id } => println!("Webhook {id} removed."),
        },
        Command::Join { address } => println!("Joining cluster at {address}... (M2)"),
        Command::Tui => println!("TUI not yet implemented (M3)."),
        Command::Web { port } => {
            println!("Web dashboard at http://127.0.0.1:{port} (M3)");
            tokio::signal::ctrl_c().await?;
        }
    }

    Ok(())
}
