use clap::Subcommand;

#[derive(Subcommand)]
pub enum AlertsAction {
    List {
        #[arg(short, long)]
        all: bool,
    },
    View {
        id: String,
    },
    Reply {
        id: String,
        message: Vec<String>,
    },
    Dismiss {
        id: String,
    },
    Fix {
        id: String,
    },
}

#[derive(Subcommand)]
pub enum SecretsAction {
    Set {
        key: String,
        value: String,
    },
    Remove {
        key: String,
    },
    List,
    Import {
        #[arg(short, long)]
        file: String,
    },
}

#[derive(Subcommand)]
pub enum ImportSource {
    DockerCompose {
        #[arg(default_value = "docker-compose.yml")]
        file: String,
        #[arg(long)]
        analyze: bool,
    },
    Coolify {
        #[arg(default_value = "/data/coolify")]
        path: String,
        #[arg(long)]
        analyze: bool,
    },
}

#[derive(Subcommand)]
pub enum BackupAction {
    Create,
    List,
    Restore { id: String },
}

#[derive(Subcommand)]
pub enum DbAction {
    Create {
        db_type: String,
        name: String,
        #[arg(long)]
        password: Option<String>,
    },
    List,
}

#[derive(Subcommand)]
pub enum WebhookAction {
    Add {
        #[arg(long)]
        repo: String,
        #[arg(long)]
        service: String,
        #[arg(long, default_value = "main")]
        branch: String,
    },
    List,
    Remove {
        id: String,
    },
}
