use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "orca", about = "Container + Wasm orchestrator with AI ops", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,

    /// API server address
    #[arg(long, default_value = "http://127.0.0.1:6880", global = true)]
    api: String,
}

#[derive(Subcommand)]
enum Command {
    /// Initialize a new cluster
    Init {
        /// Path to cluster.toml
        #[arg(short, long, default_value = "cluster.toml")]
        config: String,
    },

    /// Start the orca server (control plane + agent)
    Server {
        /// Path to cluster.toml
        #[arg(short, long, default_value = "cluster.toml")]
        config: String,
    },

    /// Start as agent only (join existing cluster)
    Agent {
        /// Control plane address to join
        #[arg(long)]
        join: String,
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
        /// Show all (including resolved/dismissed)
        #[arg(short, long)]
        all: bool,
    },
    /// View an alert conversation
    View {
        /// Alert conversation ID
        id: String,
    },
    /// Reply to an alert conversation
    Reply {
        /// Alert conversation ID
        id: String,
        /// Your message
        message: Vec<String>,
    },
    /// Dismiss an alert
    Dismiss {
        /// Alert conversation ID
        id: String,
    },
    /// Apply the AI's suggested fix for an alert
    Fix {
        /// Alert conversation ID
        id: String,
    },
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
        /// Path to docker-compose.yml
        #[arg(default_value = "docker-compose.yml")]
        file: String,
        /// Use AI to analyze and optimize the import
        #[arg(long)]
        analyze: bool,
    },
    /// Import from a Coolify installation
    Coolify {
        /// Path to Coolify data directory
        #[arg(default_value = "/data/coolify")]
        path: String,
        /// Use AI to analyze and optimize the import
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
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    match cli.command {
        Command::Init { config } => {
            tracing::info!("Initializing cluster from {config}");
            println!("orca: cluster initialized from {config}");
        }
        Command::Server { config } => {
            tracing::info!("Starting orca server from {config}");
            println!("orca: server starting...");
            tokio::signal::ctrl_c().await?;
        }
        Command::Agent { join } => {
            tracing::info!("Starting agent, joining {join}");
            tokio::signal::ctrl_c().await?;
        }
        Command::Deploy { file } => {
            tracing::info!("Deploying from {file}");
            let config = orca_core::config::ServicesConfig::load(file.as_ref())?;
            println!("orca: deploying {} services", config.service.len());
            for svc in &config.service {
                let gpu_tag = svc
                    .resources
                    .as_ref()
                    .and_then(|r| r.gpu.as_ref())
                    .map(|g| format!(" [GPU: {}x{}]", g.count, g.vendor.as_deref().unwrap_or("any")))
                    .unwrap_or_default();
                println!("  {} ({:?}){}", svc.name, svc.runtime, gpu_tag);
            }
        }
        Command::Status => {
            println!("orca: cluster status (not yet connected)");
        }
        Command::Logs {
            service,
            tail,
            follow,
            summarize,
        } => {
            if summarize {
                println!("orca: summarizing logs for {service} using AI...");
                // TODO: fetch logs → feed to AI → print summary
            } else {
                tracing::info!("Streaming logs for {service} (tail={tail}, follow={follow})");
            }
        }
        Command::Scale { service, replicas } => {
            println!("orca: scaling {service} to {replicas} replicas");
        }
        Command::Rollback { service } => {
            println!("orca: rolling back {service}");
        }

        // -- AI commands --
        Command::Ask { question } => {
            let q = question.join(" ");
            println!("orca ai: asking about cluster...\n");
            println!("Q: {q}");
            println!();
            // TODO: build ClusterContext, send to AI, stream response
            println!("(AI backend not yet connected — configure [ai] in cluster.toml)");
        }
        Command::Generate { description } => {
            let desc = description.join(" ");
            println!("orca ai: generating config for: {desc}\n");
            // TODO: send description to AI, get back TOML config
            println!("(AI backend not yet connected — configure [ai] in cluster.toml)");
        }
        Command::Alerts { action } => match action {
            AlertsAction::List { all } => {
                if all {
                    println!("orca: all alert conversations (none yet)");
                } else {
                    println!("orca: active alert conversations (none yet)");
                }
            }
            AlertsAction::View { id } => {
                println!("orca: viewing alert conversation {id}");
            }
            AlertsAction::Reply { id, message } => {
                let msg = message.join(" ");
                println!("orca: replying to alert {id}: {msg}");
            }
            AlertsAction::Dismiss { id } => {
                println!("orca: dismissing alert {id}");
            }
            AlertsAction::Fix { id } => {
                println!("orca: applying AI-suggested fix for alert {id}");
                // TODO: get the suggested_command from the alert, confirm, execute
            }
        },

        // -- GPU commands --
        Command::Gpus => {
            println!("orca: GPU status across cluster");
            println!("  (no nodes with GPUs registered yet)");
            // TODO: query nodes for GPU info, display table
        }
        Command::Nodes { gpus } => {
            if gpus {
                println!("orca: nodes with GPU details (none registered yet)");
            } else {
                println!("orca: nodes (none registered yet)");
            }
        }

        Command::Secrets { action } => match action {
            SecretsAction::Set { key, .. } => println!("orca: secret '{key}' set"),
            SecretsAction::Remove { key } => println!("orca: secret '{key}' removed"),
            SecretsAction::List => println!("orca: (no secrets yet)"),
            SecretsAction::Import { file } => println!("orca: importing secrets from {file}"),
        },
        Command::Import { source } => match source {
            ImportSource::DockerCompose { file, analyze } => {
                println!("orca: importing from docker-compose at {file}");
                if analyze {
                    println!("orca ai: analyzing import for optimizations...");
                    // TODO: AI suggests wasm conversions, resource sizing, etc.
                }
            }
            ImportSource::Coolify { path, analyze } => {
                println!("orca: importing from Coolify at {path}");
                if analyze {
                    println!("orca ai: analyzing import for optimizations...");
                }
            }
        },
        Command::Webhooks { action } => match action {
            WebhookAction::Add {
                repo,
                service,
                branch,
            } => {
                println!("orca: webhook added for {repo} -> {service} (branch: {branch})");
            }
            WebhookAction::List => println!("orca: (no webhooks yet)"),
            WebhookAction::Remove { id } => println!("orca: webhook {id} removed"),
        },
        Command::Join { address } => {
            println!("orca: joining cluster at {address}");
        }
        Command::Tui => {
            println!("orca: launching TUI...");
        }
        Command::Web { port } => {
            println!("orca: web dashboard at http://127.0.0.1:{port}");
            tokio::signal::ctrl_c().await?;
        }
    }

    Ok(())
}
