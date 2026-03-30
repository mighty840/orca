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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn try_from_http() {
        let t = Trigger::try_from("http:/api/health".to_string()).unwrap();
        assert!(matches!(t, Trigger::Http(r) if r == "/api/health"));
    }

    #[test]
    fn try_from_cron() {
        let t = Trigger::try_from("cron:0 * * * *".to_string()).unwrap();
        assert!(matches!(t, Trigger::Cron(c) if c == "0 * * * *"));
    }

    #[test]
    fn try_from_queue() {
        let t = Trigger::try_from("queue:my-topic".to_string()).unwrap();
        assert!(matches!(t, Trigger::Queue(q) if q == "my-topic"));
    }

    #[test]
    fn try_from_event() {
        let t = Trigger::try_from("event:deploy.*".to_string()).unwrap();
        assert!(matches!(t, Trigger::Event(e) if e == "deploy.*"));
    }

    #[test]
    fn try_from_invalid_returns_err() {
        let result = Trigger::try_from("bad:input".to_string());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("invalid trigger format"));
    }

    #[test]
    fn round_trip_string_trigger_string() {
        let inputs = vec![
            "http:/index",
            "cron:*/5 * * * *",
            "queue:jobs",
            "event:node.down",
        ];
        for input in inputs {
            let trigger = Trigger::try_from(input.to_string()).unwrap();
            let back: String = trigger.into();
            assert_eq!(back, input);
        }
    }
}
