//! From tool arguments to an API path, and from a resource URI to an
//! operation and its arguments.

use std::collections::BTreeMap;

use super::{OPS, Op, TAIL_DEFAULT, TAIL_MAX};

impl Op {
    /// The API path and query for these arguments, or the missing argument.
    pub fn path(&self, args: &BTreeMap<String, String>) -> Result<String, String> {
        let mut path = String::new();
        for seg in self.route.split('/').skip(1) {
            path.push('/');
            match seg.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
                Some(p) => {
                    let v = args.get(p).filter(|v| !v.is_empty());
                    let v = v.ok_or_else(|| format!("missing argument '{p}'"))?;
                    path.push_str(&encode(v));
                }
                None => path.push_str(seg),
            }
        }
        let mut query: Vec<String> = Vec::new();
        if !self.fixed_query.is_empty() {
            query.push(self.fixed_query.to_string());
        }
        for p in self
            .params
            .iter()
            .filter(|p| !self.route.contains(&format!("{{{}}}", p.name)))
        {
            let value = match (p.name, args.get(p.name)) {
                ("tail", v) => {
                    let n = v
                        .and_then(|v| v.parse::<u64>().ok())
                        .unwrap_or(TAIL_DEFAULT);
                    Some(n.clamp(1, TAIL_MAX).to_string())
                }
                (_, Some(v)) if !v.is_empty() => Some(v.clone()),
                _ => None,
            };
            if let Some(v) = value {
                query.push(format!("{}={}", p.name, encode(&v)));
            }
        }
        if !query.is_empty() {
            path.push('?');
            path.push_str(&query.join("&"));
        }
        Ok(path)
    }

    /// Whether the resource template has no `{x}` placeholder.
    pub fn is_static_resource(&self) -> bool {
        !self.resource.contains('{')
    }
}

/// The operation a resource URI names, and its arguments (path
/// placeholders, then `?k=v` query pairs).
pub fn match_resource(uri: &str) -> Option<(&'static Op, BTreeMap<String, String>)> {
    let (path, query) = uri.split_once('?').unwrap_or((uri, ""));
    let segs: Vec<&str> = path.split('/').collect();
    OPS.iter().find_map(|op| {
        let tsegs: Vec<&str> = op.resource.split('/').collect();
        if tsegs.len() != segs.len() {
            return None;
        }
        let mut args = BTreeMap::new();
        for (t, s) in tsegs.iter().zip(&segs) {
            match t.strip_prefix('{').and_then(|t| t.strip_suffix('}')) {
                Some(name) if !s.is_empty() => {
                    args.insert(name.to_string(), decode(s));
                }
                Some(_) => return None,
                None if t == s => {}
                None => return None,
            }
        }
        for pair in query.split('&').filter(|p| !p.is_empty()) {
            let (k, v) = pair.split_once('=').unwrap_or((pair, "true"));
            args.insert(k.to_string(), decode(v));
        }
        Some((op, args))
    })
}

/// Percent-encode everything but unreserved characters.
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok());
        match (bytes[i], hex.and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
