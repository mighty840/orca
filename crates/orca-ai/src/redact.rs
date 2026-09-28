//! Mask secrets in log lines before they go to the model.
//!
//! Log tails regularly contain connection strings, bearer tokens and
//! `PASSWORD=...` dumps. The model endpoint is another service, so nothing
//! that looks like a credential leaves the cluster in a prompt.

/// Key names whose values are masked in `key=value` and `key: value`.
const SECRET_KEYS: &[&str] = &[
    "password",
    "passwd",
    "pwd",
    "secret",
    "token",
    "apikey",
    "api_key",
    "api-key",
    "authorization",
    "auth",
    "credential",
    "private_key",
    "access_key",
    "session",
];

/// Shortest run of token characters treated as an opaque secret.
const OPAQUE_MIN: usize = 32;

const MASK: &str = "<redacted>";

/// Mask credentials in one log line.
pub fn redact_line(line: &str) -> String {
    let line = mask_bearer(line);
    let line = mask_url_passwords(&line);
    let line = mask_key_values(&line);
    mask_opaque(&line)
}

fn mask_bearer(line: &str) -> String {
    let lower = line.to_ascii_lowercase();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while let Some(pos) = lower[i..].find("bearer ") {
        let start = i + pos + "bearer ".len();
        let end = line[start..]
            .find(|c: char| c.is_whitespace() || c == '"' || c == '\'')
            .map_or(line.len(), |e| start + e);
        out.push_str(&line[i..start]);
        out.push_str(MASK);
        i = end;
    }
    out.push_str(&line[i..]);
    out
}

/// `scheme://user:password@host` keeps the user, masks the password.
fn mask_url_passwords(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(pos) = rest.find("://") {
        let (head, tail) = rest.split_at(pos + 3);
        out.push_str(head);
        let authority_end = tail
            .find(|c: char| c == '/' || c.is_whitespace() || c == '"')
            .unwrap_or(tail.len());
        let authority = &tail[..authority_end];
        match (authority.rfind('@'), authority.find(':')) {
            (Some(at), Some(colon)) if colon < at => {
                out.push_str(&authority[..=colon]);
                out.push_str(MASK);
                out.push_str(&authority[at..]);
            }
            _ => out.push_str(authority),
        }
        rest = &tail[authority_end..];
    }
    out.push_str(rest);
    out
}

fn mask_key_values(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let bytes = line.as_bytes();
    let mut i = 0;
    let mut copied = 0;
    while i < bytes.len() {
        let sep = bytes[i];
        if sep == b'=' || sep == b':' {
            let key_start = line[..i]
                .rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '"'))
                .map_or(0, |p| p + 1);
            let key = line[key_start..i].trim_matches('"').to_ascii_lowercase();
            if SECRET_KEYS.iter().any(|k| key.ends_with(k)) {
                let mut vstart = i + 1;
                while vstart < bytes.len() && (bytes[vstart] == b' ' || bytes[vstart] == b'"') {
                    vstart += 1;
                }
                let vend = line[vstart..]
                    .find(|c: char| {
                        c.is_whitespace() || c == '"' || c == ',' || c == '&' || c == ';'
                    })
                    .map_or(line.len(), |e| vstart + e);
                if vend > vstart {
                    out.push_str(&line[copied..vstart]);
                    out.push_str(MASK);
                    copied = vend;
                    i = vend;
                    continue;
                }
            }
        }
        i += 1;
    }
    out.push_str(&line[copied..]);
    out
}

/// Long runs that mix letters and digits look like keys or tokens. Hex-only
/// runs (digests, commit hashes, request ids) are kept: they are evidence.
/// `/` ends a run, so long file paths survive.
fn mask_opaque(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        let hex = run.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
        let letters = run.chars().any(|c| c.is_ascii_alphabetic());
        let digits = run.chars().any(|c| c.is_ascii_digit());
        if run.len() >= OPAQUE_MIN && letters && digits && !hex {
            out.push_str(MASK);
        } else {
            out.push_str(run);
        }
        run.clear();
    };
    for c in line.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '+' {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::redact_line;

    #[test]
    fn masks_key_values() {
        assert_eq!(
            redact_line("POSTGRES_PASSWORD=hunter2 user=app"),
            "POSTGRES_PASSWORD=<redacted> user=app"
        );
        assert_eq!(
            redact_line(r#"{"api_key": "abc123", "model": "x"}"#),
            r#"{"api_key": "<redacted>", "model": "x"}"#
        );
    }

    #[test]
    fn masks_bearer_and_url_passwords() {
        assert_eq!(
            redact_line("Authorization: Bearer eyJhbGciOi.x.y failed"),
            "Authorization: <redacted> <redacted> failed"
        );
        assert_eq!(
            redact_line("connect postgres://app:s3cret@db:5432/app refused"),
            "connect postgres://app:<redacted>@db:5432/app refused"
        );
    }

    #[test]
    fn masks_opaque_tokens_but_keeps_digests() {
        let digest = "sha256:9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";
        assert_eq!(redact_line(digest), digest);
        assert_eq!(
            redact_line("using key sk-proj-4fQ9xZ2mL8vT1bN6cR3yW7kP0aE5sD9h"),
            "using key <redacted>"
        );
    }

    #[test]
    fn leaves_ordinary_lines_alone() {
        let line = "2026-09-26T10:17:03Z ERROR exporter: code: 210, message: Connection refused (clickhouse:9000)";
        assert_eq!(redact_line(line), line);
    }
}
