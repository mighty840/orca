//! Extract orca CLI commands from AI response text.

/// Extract an `orca ...` command from the AI's response, if present. A
/// structured diagnosis quotes commands in several sections; the one to run
/// is in **Fix**, so that section is searched first.
pub(crate) fn extract_command(content: &str) -> (Option<String>, String) {
    if let Some(fix) = fix_section(content)
        && let Some(cmd) = first_command(fix)
    {
        return (Some(cmd), content.to_string());
    }
    (first_command(content), content.to_string())
}

/// The text from a "Fix" heading up to the next bold or `#` heading.
fn fix_section(content: &str) -> Option<&str> {
    let start = content.find("**Fix**").or_else(|| content.find("## Fix"))?;
    let body = &content[start..];
    let after_heading = body.find('\n').map_or(body.len(), |n| n + 1);
    let end = body[after_heading..]
        .find("\n**")
        .or_else(|| body[after_heading..].find("\n#"))
        .map_or(body.len(), |e| after_heading + e);
    Some(&body[..end])
}

fn first_command(content: &str) -> Option<String> {
    for line in content.lines() {
        // Check if the whole line is a command (possibly backtick-wrapped)
        let trimmed = line.trim().trim_start_matches('`').trim_end_matches('`');
        if trimmed.starts_with("orca ") {
            return Some(trimmed.to_string());
        }
        // Check for inline backtick-wrapped commands: `orca ...`
        if let Some(start) = line.find("`orca ") {
            let rest = &line[start + 1..];
            if let Some(end) = rest.find('`') {
                return Some(rest[..end].to_string());
            }
        }
    }
    None
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
    fn test_extract_command_prefers_the_fix_section() {
        let response = "**What happened**: api OOM-killed.\n\
            **Evidence**: exit 137 (see `orca logs api --tail 50`).\n\
            **Fix**: raise `memory` to 1Gi in services/app/service.toml, then\n\
            `orca deploy api`\n\
            **Verify**: `orca status`";
        let (cmd, _content) = extract_command(response);
        assert_eq!(cmd.unwrap(), "orca deploy api");
    }

    #[test]
    fn test_extract_command_no_command_returns_none() {
        let response = "Everything looks fine. No action needed at this time.";
        let (cmd, content) = extract_command(response);
        assert!(cmd.is_none());
        assert_eq!(content, response);
    }
}
