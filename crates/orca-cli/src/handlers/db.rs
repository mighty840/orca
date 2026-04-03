//! One-click database provisioning via the orca API.

use std::collections::HashMap;

use anyhow::{Result, bail};
use orca_core::config::ServiceConfig;
use orca_core::secrets::SecretStore;
use orca_core::types::VolumeSpec;

use crate::client::OrcaClient;
use crate::commands::DbAction;

/// Handle database subcommands.
pub async fn handle_db(action: DbAction, api: String) -> Result<()> {
    let client = OrcaClient::new(api);

    match action {
        DbAction::Create {
            db_type,
            name,
            password,
        } => handle_create(&client, &db_type, &name, password).await,
        DbAction::List => handle_list(&client).await,
    }
}

async fn handle_create(
    client: &OrcaClient,
    db_type: &str,
    name: &str,
    password: Option<String>,
) -> Result<()> {
    let password = password.unwrap_or_else(|| generate_password(24));

    let (image, port, env, health) = match db_type {
        "postgres" => (
            "postgres:16",
            5432u16,
            HashMap::from([
                ("POSTGRES_PASSWORD".into(), password.clone()),
                ("POSTGRES_DB".into(), name.into()),
            ]),
            Some("/healthz".to_string()),
        ),
        "mysql" => (
            "mysql:8",
            3306u16,
            HashMap::from([
                ("MYSQL_ROOT_PASSWORD".into(), password.clone()),
                ("MYSQL_DATABASE".into(), name.into()),
            ]),
            None,
        ),
        "redis" => ("redis:7-alpine", 6379u16, HashMap::new(), None),
        "mongodb" => (
            "mongo:7",
            27017u16,
            HashMap::from([
                ("MONGO_INITDB_ROOT_USERNAME".into(), "root".into()),
                ("MONGO_INITDB_ROOT_PASSWORD".into(), password.clone()),
            ]),
            None,
        ),
        other => bail!("unsupported database type: {other}. Use: postgres, mysql, redis, mongodb"),
    };

    let service = ServiceConfig {
        name: name.to_string(),
        project: None,
        runtime: Default::default(),
        image: Some(image.to_string()),
        module: None,
        replicas: Default::default(),
        port: Some(port),
        host_port: None,
        domain: None,
        routes: vec![],
        health,
        readiness: None,
        liveness: None,
        env,
        resources: None,
        volume: Some(VolumeSpec {
            path: format!("/var/lib/{db_type}"),
            size: None,
        }),
        deploy: None,
        placement: None,
        network: None,
        aliases: vec![],
        mounts: vec![],
        triggers: vec![],
        assets: None,
        build: None,
        tls_cert: None,
        tls_key: None,
        internal: false,
        depends_on: vec![],
    };

    // Store password as a secret
    let secrets_path = dirs_next::data_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("orca")
        .join("secrets.json");
    let mut store = SecretStore::open(&secrets_path)?;
    let secret_key = format!("{name}_password");
    store.set(&secret_key, &password)?;

    // Deploy via API
    let services = orca_core::config::ServicesConfig {
        service: vec![service],
    };
    client.deploy(&services).await?;

    let conn_string = match db_type {
        "postgres" => format!("postgres://postgres:{password}@localhost:{port}/{name}"),
        "mysql" => format!("mysql://root:{password}@localhost:{port}/{name}"),
        "redis" => format!("redis://localhost:{port}"),
        "mongodb" => format!("mongodb://root:{password}@localhost:{port}"),
        _ => String::new(),
    };

    println!("Database '{name}' ({db_type}) deployed successfully.");
    println!("Connection string: {conn_string}");
    println!("Password stored as secret: {secret_key}");

    Ok(())
}

async fn handle_list(client: &OrcaClient) -> Result<()> {
    let status = client.status().await?;
    let db_images = ["postgres", "mysql", "redis", "mongo", "mariadb"];

    println!("{:<20} {:<15} {:<10}", "NAME", "IMAGE", "STATUS");
    println!("{}", "-".repeat(45));

    for svc in &status.services {
        let is_db = db_images.iter().any(|img| svc.image.contains(img));
        if is_db {
            println!("{:<20} {:<15} {:<10}", svc.name, svc.image, svc.status);
        }
    }

    Ok(())
}

fn generate_password(len: usize) -> String {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};

    const CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut result = String::with_capacity(len);

    for i in 0..len {
        let state = RandomState::new();
        let mut hasher = state.build_hasher();
        hasher.write_usize(i);
        let idx = hasher.finish() as usize % CHARSET.len();
        result.push(CHARSET[idx] as char);
    }

    result
}
