//! The heartbeat's log tail must never panic on the log's content (#180).

use proptest::prelude::*;

use super::*;

#[test]
fn a_short_log_is_returned_whole() {
    assert_eq!(bounded_tail("boom", 4096), "boom");
}

#[test]
fn a_long_ascii_log_keeps_its_last_bytes() {
    let log = "x".repeat(5000) + "END";
    let out = bounded_tail(&log, 10);
    assert_eq!(out, format!("…{}", &log[log.len() - 10..]));
    assert!(out.ends_with("END"));
}

#[test]
fn a_cut_inside_a_multibyte_character_does_not_panic() {
    // "€" is three bytes and 4096 is not a multiple of three, so the last
    // 4096 bytes of a run of them starts inside a character. (Two-byte
    // characters would not do: 4096 bytes of them is always whole.)
    let log = "€".repeat(2000);
    let naive_cut = log.len() - LOG_TAIL_LIMIT;
    assert!(
        !log.is_char_boundary(naive_cut),
        "the test must exercise a mid-character cut"
    );
    let out = bounded_tail(&log, LOG_TAIL_LIMIT);
    assert!(out.starts_with('…'));
    assert!(out.ends_with('€'));
}

proptest! {
    /// No log content can make the tail panic, the result never exceeds the
    /// limit plus the ellipsis, and it is always a suffix of the log.
    #[test]
    fn bounded_tail_is_safe_for_any_log(text in any::<String>(), limit in 0usize..64) {
        let out = bounded_tail(&text, limit);
        if text.len() <= limit {
            prop_assert_eq!(&out, &text);
        } else {
            let body = out.strip_prefix('…').expect("a cut is marked");
            prop_assert!(body.len() <= limit);
            prop_assert!(text.ends_with(body));
        }
    }
}
