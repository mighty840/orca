mod client;
mod commands;
mod handlers;
mod subcommands;

use clap::Parser;

use commands::Command;

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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    // Set up structured logging — console + optional file output for crash analysis.
    // Set ORCA_LOG_FILE=path to also log to a file in JSON format.
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "info,hyper=warn,reqwest=warn".into());

    if let Ok(log_path) = std::env::var("ORCA_LOG_FILE") {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .expect("failed to open log file");
        let file_layer = tracing_subscriber::fmt::layer()
            .json()
            .with_writer(std::sync::Mutex::new(file));
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;
        tracing_subscriber::registry()
            .with(env_filter)
            .with(tracing_subscriber::fmt::layer())
            .with(file_layer)
            .init();
    } else {
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
    };

    match cli.command {
        Command::Server {
            config,
            proxy_port,
            daemon,
        } => {
            if daemon {
                let args: Vec<String> = std::env::args().skip(1).collect();
                handlers::daemon::daemonize(&args)?;
                return Ok(());
            }
            handlers::server::handle_server(&config, proxy_port).await?;
        }
        Command::Deploy { file } => {
            handlers::deploy::handle_deploy(&file, cli.api).await?;
        }
        Command::Status => {
            handlers::status::handle_status(cli.api).await?;
        }
        Command::Logs {
            service,
            tail,
            follow: _,
            summarize,
        } => {
            handlers::ops::handle_logs(service, tail, summarize, cli.api).await?;
        }
        Command::Scale { service, replicas } => {
            handlers::ops::handle_scale(service, replicas, cli.api).await?;
        }
        Command::Ask { question } => handlers::ops::handle_ask(question),
        Command::Generate { description } => handlers::ops::handle_generate(description),
        Command::Alerts { action } => handlers::ops::handle_alerts(action),
        Command::Secrets { action } => handlers::ops::handle_secrets(action),
        Command::Import { source } => handlers::import::handle_import(source),
        Command::Webhooks { action } => handlers::ops::handle_webhooks(action, cli.api).await?,
        Command::Nodes { gpus } => handlers::ops::handle_nodes(gpus, cli.api).await?,
        Command::Gpus => handlers::ops::handle_gpus(),
        Command::Reload => handlers::reload::handle_reload().await?,
        Command::Exec { service, cmd } => {
            handlers::exec::handle_exec(&service, &cmd)?;
        }
        Command::Stop { service } => {
            handlers::ops::handle_stop(service, cli.api).await?;
        }
        Command::Rollback { service } => {
            handlers::ops::handle_rollback(service, cli.api).await?;
        }
        Command::Token => handlers::server::show_token(),
        Command::Join {
            address,
            token,
            daemon,
            setup_key,
        } => {
            if daemon {
                let args: Vec<String> = std::env::args().skip(1).collect();
                handlers::daemon::daemonize(&args)?;
                return Ok(());
            }
            // SAFETY: single-threaded at this point, before tokio runtime starts work
            unsafe { std::env::set_var("ORCA_TOKEN", &token) };
            handlers::join::handle_join(
                &address,
                None,
                std::collections::HashMap::new(),
                setup_key,
            )
            .await?;
        }
        Command::Backup { action } => handlers::backup::handle_backup(action),
        Command::Cleanup => {
            handlers::cleanup::handle_cleanup().await?;
        }
        Command::Shutdown => {
            handlers::daemon::handle_shutdown()?;
        }
        Command::Db { action } => {
            handlers::db::handle_db(action, cli.api).await?;
        }
        Command::Build { service, file } => {
            handlers::build::handle_build(&file, service).await?;
        }
        Command::Tui => handlers::ops::handle_tui(&cli.api).await?,
        Command::Web { port } => handlers::ops::handle_web(port).await?,
    }

    Ok(())
}
