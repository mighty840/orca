use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum Trigger {
    Http(String),
    Cron(String),
    Queue(String),
    Event(String),
}

impl TryFrom<String> for Trigger {
    type Error = String;
    fn try_from(s: String) -> std::result::Result<Self, Self::Error> {
        if let Some(route) = s.strip_prefix("http:") {
            Ok(Trigger::Http(route.to_string()))
        } else if let Some(cron) = s.strip_prefix("cron:") {
            Ok(Trigger::Cron(cron.to_string()))
        } else if let Some(topic) = s.strip_prefix("queue:") {
            Ok(Trigger::Queue(topic.to_string()))
        } else if let Some(pattern) = s.strip_prefix("event:") {
            Ok(Trigger::Event(pattern.to_string()))
        } else {
            Err(format!("invalid trigger format: {s}"))
        }
    }
}

impl From<Trigger> for String {
    fn from(t: Trigger) -> Self {
        match t {
            Trigger::Http(r) => format!("http:{r}"),
            Trigger::Cron(c) => format!("cron:{c}"),
            Trigger::Queue(q) => format!("queue:{q}"),
            Trigger::Event(e) => format!("event:{e}"),
        }
    }
}
