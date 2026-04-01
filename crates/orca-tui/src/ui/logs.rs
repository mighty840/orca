//! Log panel rendering with syntax highlighting and word wrap.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::state::{AppState, Panel};

/// Draw the logs panel with line numbers, auto-scroll, and syntax highlighting.
pub fn draw_logs(f: &mut Frame, area: Rect, state: &AppState) {
    let border_style = if state.panel == Panel::Logs {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let log_lines: Vec<&str> = state.logs.lines().collect();
    let total = log_lines.len();
    let wrap_indicator = if state.word_wrap { " wrap" } else { "" };

    let title = state
        .selected_service_name()
        .map(|n| format!(" Logs: {n} ({total} lines){wrap_indicator} "))
        .unwrap_or_else(|| format!(" Logs ({total} lines){wrap_indicator} "));

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(border_style);

    let inner_w = if area.width > 2 {
        (area.width - 2) as usize
    } else {
        0
    };
    let inner_h = if area.height > 2 {
        (area.height - 2) as usize
    } else {
        0
    };

    // Auto-scroll: show the last `inner_h` lines.
    let start = total.saturating_sub(inner_h);
    let line_num_width = format!("{}", total).len();

    let lines: Vec<Line> = log_lines[start..]
        .iter()
        .enumerate()
        .map(|(i, text)| {
            let num = start + i + 1;
            let num_str = format!("{:>width$} ", num, width = line_num_width);

            let max_text = if !state.word_wrap && inner_w > num_str.len() {
                inner_w - num_str.len()
            } else if state.word_wrap {
                text.len() // no truncation
            } else {
                0
            };
            let display_text: String = if state.word_wrap {
                (*text).to_string()
            } else {
                text.chars().take(max_text).collect()
            };

            let mut spans = vec![Span::styled(
                num_str,
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM),
            )];

            spans.extend(highlight_log_line(&display_text));
            Line::from(spans)
        })
        .collect();

    let mut para = Paragraph::new(lines).block(block);
    if state.word_wrap {
        para = para.wrap(Wrap { trim: false });
    }
    f.render_widget(para, area);
}

/// Apply syntax highlighting to a log line.
fn highlight_log_line(text: &str) -> Vec<Span<'static>> {
    // Try to detect and color log level keywords
    let upper = text.to_uppercase();

    // Check for log levels
    let level_color =
        if upper.contains("ERROR") || upper.contains("FATAL") || upper.contains("PANIC") {
            Some(Color::Red)
        } else if upper.contains("WARN") {
            Some(Color::Yellow)
        } else if upper.contains("INFO") {
            Some(Color::Green)
        } else if upper.contains("DEBUG") || upper.contains("TRACE") {
            Some(Color::DarkGray)
        } else {
            None
        };

    // Check for HTTP status codes (3-digit numbers)
    let http_color = detect_http_status(text);

    // Try to split out a leading timestamp
    let (ts, rest) = split_timestamp(text);

    let mut spans: Vec<Span<'static>> = Vec::new();

    if let Some(ts_str) = ts {
        spans.push(Span::styled(ts_str, Style::default().fg(Color::DarkGray)));
    }

    let body = rest.to_string();
    let body_style = if let Some(c) = level_color {
        Style::default().fg(c)
    } else if let Some(c) = http_color {
        Style::default().fg(c)
    } else {
        Style::default().fg(Color::White)
    };

    spans.push(Span::styled(body, body_style));
    spans
}

/// Detect HTTP status codes in text and return appropriate color.
fn detect_http_status(text: &str) -> Option<Color> {
    // Look for patterns like " 200 ", " 404 ", " 500 " etc.
    for word in text.split_whitespace() {
        if word.len() == 3
            && let Ok(code) = word.parse::<u16>()
        {
            return match code {
                200..=299 => Some(Color::Green),
                300..=399 => Some(Color::Cyan),
                400..=499 => Some(Color::Yellow),
                500..=599 => Some(Color::Red),
                _ => None,
            };
        }
    }
    None
}

/// Try to split a leading timestamp from a log line.
/// Returns (Some(timestamp_string), rest) or (None, full_line).
fn split_timestamp(text: &str) -> (Option<String>, &str) {
    // Common patterns: "2024-01-15T10:30:00" or "2024-01-15 10:30:00"
    // or "[2024-01-15T10:30:00Z]"
    let bytes = text.as_bytes();

    // Check for ISO timestamp at start: YYYY-MM-DD
    if bytes.len() >= 19 && bytes[4] == b'-' && bytes[7] == b'-' {
        // Find end of timestamp (up to space after time or 'Z' or ']')
        let end = text[10..]
            .find([' ', ']'])
            .map(|i| i + 10 + 1)
            .unwrap_or(19);
        let end = end.min(text.len());
        return (Some(text[..end].to_string()), &text[end..]);
    }

    // Check for bracketed timestamp: [YYYY-...] or [HH:MM:SS]
    if bytes.first() == Some(&b'[')
        && let Some(close) = text.find(']')
    {
        let end = (close + 1).min(text.len());
        return (Some(text[..end].to_string()), &text[end..]);
    }

    (None, text)
}
