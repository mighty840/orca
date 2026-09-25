//! Memory limit strings such as `512Mi` or `4Gi`.

const HINT: &str = "use a byte count or a unit such as \"512Mi\" or \"4Gi\"";

/// Parse a memory limit into bytes.
///
/// Accepts a plain byte count, binary units `Ki`/`Mi`/`Gi`/`Ti` (powers of
/// 1024) and decimal units `K`/`M`/`G`/`T` (powers of 1000, optionally with a
/// trailing `B`, as in `4GB`), with an optional fraction (`1.5Gi`). Anything
/// else is an error. An unparseable limit used to become "no limit" (#177),
/// so a typo like `4g` silently removed a service's memory cap. `0` stays
/// valid and means "no limit", as it does for Docker.
pub fn parse_memory_bytes(s: &str) -> Result<i64, String> {
    let t = s.trim();
    let split = t
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(t.len());
    let (num, unit) = t.split_at(split);
    let multiplier: f64 = match unit {
        "" | "B" => 1.0,
        "Ki" => 1024.0,
        "Mi" => 1024.0 * 1024.0,
        "Gi" => 1024.0 * 1024.0 * 1024.0,
        "Ti" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        "K" | "KB" => 1e3,
        "M" | "MB" => 1e6,
        "G" | "GB" => 1e9,
        "T" | "TB" => 1e12,
        _ => {
            return Err(format!(
                "invalid memory limit {s:?}: unknown unit {unit:?}; {HINT}"
            ));
        }
    };
    let n: f64 = num
        .parse()
        .map_err(|_| format!("invalid memory limit {s:?}: {HINT}"))?;
    let bytes = (n * multiplier).round();
    if bytes > i64::MAX as f64 {
        return Err(format!("invalid memory limit {s:?}: out of range"));
    }
    Ok(bytes as i64)
}

#[cfg(test)]
mod tests {
    use super::parse_memory_bytes as parse;

    #[test]
    fn binary_decimal_and_plain_units() {
        assert_eq!(parse("512Mi"), Ok(512 * 1024 * 1024));
        assert_eq!(parse("4Gi"), Ok(4 * 1024 * 1024 * 1024));
        assert_eq!(parse("1.5Gi"), Ok(1536 * 1024 * 1024));
        assert_eq!(parse("64Ki"), Ok(65536));
        assert_eq!(parse("4G"), Ok(4_000_000_000));
        assert_eq!(parse("4GB"), Ok(4_000_000_000));
        assert_eq!(parse("512M"), Ok(512_000_000));
        assert_eq!(parse("1048576"), Ok(1_048_576));
        assert_eq!(parse(" 256Mi "), Ok(256 * 1024 * 1024));
        assert_eq!(parse("0"), Ok(0), "Docker's explicit no-limit");
    }

    #[test]
    fn typos_are_errors_not_unlimited() {
        for bad in ["4g", "4 Gi", "4GiB", "Gi", "", "-1Gi", "4.2.1Gi", "lots"] {
            assert!(parse(bad).is_err(), "{bad:?} must be rejected");
        }
    }
}
