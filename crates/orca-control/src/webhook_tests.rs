use super::*;

#[test]
fn branch_from_ref_extracts_name() {
    assert_eq!(branch_from_ref("refs/heads/main"), Some("main"));
    assert_eq!(branch_from_ref("refs/heads/feat/foo"), Some("feat/foo"));
    assert_eq!(branch_from_ref("refs/tags/v1.0"), None);
}

#[test]
fn validate_signature_works() {
    let secret = "mysecret";
    let body = b"hello world";
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(body);
    let result = mac.finalize().into_bytes();
    let sig = format!("sha256={}", hex::encode(result));
    assert!(validate_signature(secret, body, &sig));
    assert!(!validate_signature(secret, body, "sha256=badbeef"));
    assert!(!validate_signature(secret, body, "invalid"));
}

#[tokio::test]
async fn webhook_store_remove_by_service_name() {
    let store = new_store();
    {
        let mut webhooks = store.write().await;
        webhooks.push(WebhookConfig {
            repo: "org/api".to_string(),
            service_name: "api".to_string(),
            branch: "main".to_string(),
            secret: None,
        });
        webhooks.push(WebhookConfig {
            repo: "org/web".to_string(),
            service_name: "web".to_string(),
            branch: "main".to_string(),
            secret: None,
        });
        webhooks.push(WebhookConfig {
            repo: "org/api".to_string(),
            service_name: "api".to_string(),
            branch: "develop".to_string(),
            secret: None,
        });
    }

    // Remove all webhooks for "api"
    {
        let mut webhooks = store.write().await;
        webhooks.retain(|w| w.service_name != "api");
    }

    let webhooks = store.read().await;
    assert_eq!(webhooks.len(), 1);
    assert_eq!(webhooks[0].service_name, "web");
}

#[tokio::test]
async fn webhook_store_remove_nonexistent() {
    let store = new_store();
    {
        let mut webhooks = store.write().await;
        webhooks.push(WebhookConfig {
            repo: "org/api".to_string(),
            service_name: "api".to_string(),
            branch: "main".to_string(),
            secret: None,
        });
    }

    {
        let mut webhooks = store.write().await;
        let before = webhooks.len();
        webhooks.retain(|w| w.service_name != "nonexistent");
        assert_eq!(webhooks.len(), before); // nothing removed
    }

    let webhooks = store.read().await;
    assert_eq!(webhooks.len(), 1);
}

#[test]
fn webhook_config_serialization() {
    let config = WebhookConfig {
        repo: "org/app".to_string(),
        service_name: "app".to_string(),
        branch: "main".to_string(),
        secret: None,
    };
    let json = serde_json::to_string(&config).unwrap();
    let parsed: WebhookConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.repo, "org/app");
    assert_eq!(parsed.service_name, "app");
    assert_eq!(parsed.branch, "main");
    assert!(parsed.secret.is_none());
}
