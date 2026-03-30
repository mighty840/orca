//! Extract orca CLI commands from AI response text.

/// Extract an `orca ...` command from the AI's response, if present.
pub(crate) fn extract_command(content: &str) -> (Option<String>, String) {
    for line in content.lines() {
        // Check if the whole line is a command (possibly backtick-wrapped)
        let trimmed = line.trim().trim_start_matches('`').trim_end_matches('`');
        if trimmed.starts_with("orca ") {
            return (Some(trimmed.to_string()), content.to_string());
        }
        // Check for inline backtick-wrapped commands: `orca ...`
        if let Some(start) = line.find("`orca ") {
            let rest = &line[start + 1..];
            if let Some(end) = rest.find('`') {
                let cmd = &rest[..end];
                return (Some(cmd.to_string()), content.to_string());
            }
        }
    }
    (None, content.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_command_plain_orca_command() {
        let response = "The service is overloaded. I recommend scaling it up:\norca scale api 5\nThis should help with the load.";
        let (cmd, _content) = extract_command(response);
        assert_eq!(cmd.unwrap(), "orca scale api 5");
    }

    #[test]
    fn test_extract_command_backtick_wrapped() {
        let response = "Try running `orca config set max-replicas 10` to increase the limit.";
        let (cmd, _content) = extract_command(response);
        assert_eq!(cmd.unwrap(), "orca config set max-replicas 10");
    }

    #[test]
    fn test_extract_command_no_command_returns_none() {
        let response = "Everything looks fine. No action needed at this time.";
        let (cmd, content) = extract_command(response);
        assert!(cmd.is_none());
        assert_eq!(content, response);
    }
}
