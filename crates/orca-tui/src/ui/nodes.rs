//! Full-screen node table (k9s style).

use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Paragraph, Row, Table};

use crate::state::AppState;

/// Draw the full-screen nodes table with status coloring.
pub fn draw_nodes(f: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default()
        .title(format!(" Nodes ({}) ", state.nodes.len()))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    if state.nodes.is_empty() {
        let para = Paragraph::new("  No nodes registered (single-node mode)").block(block);
        f.render_widget(para, area);
        return;
    }

    let rows: Vec<Row> = state
        .nodes
        .iter()
        .map(|n| {
            let (relative, stale) = format_relative_heartbeat(&n.last_heartbeat);
            let status_color = if stale { Color::DarkGray } else { Color::Green };
            let status_text = if stale { "stale" } else { "connected" };

            Row::new(vec![
                n.node_id.to_string(),
                n.address.clone(),
                status_text.to_string(),
                relative,
            ])
            .style(Style::default().fg(status_color))
        })
        .collect();

    let header = Row::new(vec!["ID", "ADDRESS", "STATUS", "LAST HEARTBEAT"]).style(
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );
    let widths = [
        Constraint::Length(8),
        Constraint::Min(25),
        Constraint::Length(12),
        Constraint::Min(16),
    ];
    let table = Table::new(rows, widths).header(header).block(block);
    f.render_widget(table, area);
}

/// Parse an ISO 8601 heartbeat timestamp and return relative time + staleness.
fn format_relative_heartbeat(ts: &str) -> (String, bool) {
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    if let Some(ts_secs) = parse_iso_timestamp(ts) {
        let diff = now_secs.saturating_sub(ts_secs);
        let stale = diff > 30;
        let relative = if diff < 60 {
            format!("{diff}s ago")
        } else if diff < 3600 {
            format!("{}m ago", diff / 60)
        } else {
            format!("{}h ago", diff / 3600)
        };
        (relative, stale)
    } else {
        (ts.chars().take(19).collect(), false)
    }
}

/// Minimal ISO 8601 parser -> unix seconds. Handles "YYYY-MM-DDTHH:MM:SS".
fn parse_iso_timestamp(ts: &str) -> Option<u64> {
    let ts = ts.trim_end_matches('Z').trim();
    if ts.len() < 19 {
        return None;
    }
    let year: u64 = ts[0..4].parse().ok()?;
    let month: u64 = ts[5..7].parse().ok()?;
    let day: u64 = ts[8..10].parse().ok()?;
    let hour: u64 = ts[11..13].parse().ok()?;
    let min: u64 = ts[14..16].parse().ok()?;
    let sec: u64 = ts[17..19].parse().ok()?;

    let mut days: u64 = 0;
    for y in 1970..year {
        days += if is_leap(y) { 366 } else { 365 };
    }
    let month_days = [0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    for m in 1..month {
        days += month_days[m as usize];
        if m == 2 && is_leap(year) {
            days += 1;
        }
    }
    days += day - 1;

    Some(days * 86400 + hour * 3600 + min * 60 + sec)
}

fn is_leap(y: u64) -> bool {
    (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400)
}
