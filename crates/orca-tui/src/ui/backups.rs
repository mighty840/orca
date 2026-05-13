//! Backups view — per-node snapshot summary table.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Row, Table};

use crate::api::{ClusterBackupsResponse, NodeBackupStatus, NodeRole};
use crate::state::AppState;

pub fn draw_backups(f: &mut Frame, area: Rect, state: &AppState) {
    let Some(resp) = &state.backups else {
        let block = Block::default()
            .title(" Backups ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));
        let para = Paragraph::new("  Loading cluster backup status...  (press 'r' to refresh)")
            .block(block);
        f.render_widget(para, area);
        return;
    };

    if resp.nodes.is_empty() {
        let block = Block::default()
            .title(" Backups (0) ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));
        let para = Paragraph::new("  No nodes reporting backup status.").block(block);
        f.render_widget(para, area);
        return;
    }

    // Split the area: table on top, selected-node detail underneath. Detail
    // exists so the operator can read the full last-failure message without
    // the table being awkwardly wide.
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(6), Constraint::Length(6)])
        .split(area);

    draw_table(f, chunks[0], state, resp);
    draw_detail(f, chunks[1], state, resp);
}

fn draw_table(f: &mut Frame, area: Rect, state: &AppState, resp: &ClusterBackupsResponse) {
    let header = Row::new(vec![
        "ROLE",
        "HOSTNAME",
        "LAST RUN",
        "SNAPSHOTS",
        "TOTAL SIZE",
        "LAST RESULT",
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));

    let rows: Vec<Row> = resp
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let selected = i == state.selected_backup_node;
            let style = if selected {
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            Row::new(node_row(n)).style(style)
        })
        .collect();

    let widths = [
        Constraint::Length(7),
        Constraint::Length(24),
        Constraint::Length(20),
        Constraint::Length(10),
        Constraint::Length(12),
        Constraint::Min(20),
    ];

    let block = Block::default()
        .title(format!(" Backups ({}) ", resp.nodes.len()))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let table = Table::new(rows, widths).header(header).block(block);
    f.render_widget(table, area);
}

fn node_row(n: &NodeBackupStatus) -> Vec<Span<'static>> {
    let role = match n.role {
        NodeRole::Master => "master",
        NodeRole::Agent => "agent",
    };

    let (last_run, snapshot_count, total_size) = if let Some(first) = n.snapshots.first() {
        (
            format_relative_age(first.epoch_secs),
            n.snapshots.len().to_string(),
            human_bytes(n.snapshots.iter().map(|s| s.total_size_bytes).sum()),
        )
    } else {
        ("never".to_string(), "0".to_string(), "-".to_string())
    };

    let (last_result_text, last_result_color) = last_result_cell(n);

    vec![
        Span::raw(role.to_string()),
        Span::raw(n.hostname.clone()),
        Span::raw(last_run),
        Span::raw(snapshot_count),
        Span::raw(total_size),
        Span::styled(last_result_text, Style::default().fg(last_result_color)),
    ]
}

fn last_result_cell(n: &NodeBackupStatus) -> (String, Color) {
    if !n.reachable {
        return ("unreachable".into(), Color::Yellow);
    }
    match &n.last_result {
        None => ("—".into(), Color::Gray),
        Some(r) if r.success => ("ok".into(), Color::Green),
        Some(r) => (truncate(&r.message, 40), Color::Red),
    }
}

fn draw_detail(f: &mut Frame, area: Rect, state: &AppState, resp: &ClusterBackupsResponse) {
    let Some(n) = resp.nodes.get(state.selected_backup_node) else {
        return;
    };
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("Selected: ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(n.hostname.clone()),
    ]));
    if let Some(first) = n.snapshots.first() {
        lines.push(Line::from(format!(
            "Latest snapshot: {} files, {} total",
            first.files.len(),
            human_bytes(first.total_size_bytes),
        )));
    }
    if let Some(r) = &n.last_result {
        let color = if r.success { Color::Green } else { Color::Red };
        lines.push(Line::from(vec![
            Span::styled(
                "Last result: ",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(r.message.clone(), Style::default().fg(color)),
        ]));
        lines.push(Line::from(format!(
            "Recorded at: {}",
            r.recorded_at.format("%Y-%m-%d %H:%M:%S UTC"),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            "No backup result reported this session.",
            Style::default().fg(Color::Gray),
        )));
    }

    let block = Block::default()
        .title(" Detail ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

pub(super) fn format_relative_age(epoch_secs: u64) -> String {
    let Some(then) = chrono::DateTime::<chrono::Utc>::from_timestamp(epoch_secs as i64, 0) else {
        return "—".into();
    };
    let delta = chrono::Utc::now().signed_duration_since(then);
    let secs = delta.num_seconds();
    if secs < 0 {
        return then.format("%Y-%m-%d %H:%M").to_string();
    }
    if secs < 60 {
        return format!("{secs}s ago");
    }
    let mins = secs / 60;
    if mins < 60 {
        return format!("{mins}m ago");
    }
    let hours = mins / 60;
    if hours < 24 {
        return format!("{hours}h ago");
    }
    let days = hours / 24;
    if days < 30 {
        return format!("{days}d ago");
    }
    then.format("%Y-%m-%d").to_string()
}

pub(super) fn human_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KiB", "MiB", "GiB", "TiB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[0])
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `format_relative_age` collapses small durations into compact units —
    /// the dashboard column is narrow and full timestamps would push other
    /// columns out of view. We verify each branch is selected at the right
    /// threshold.
    #[test]
    fn format_relative_age_chooses_unit_by_magnitude() {
        let now = chrono::Utc::now().timestamp() as u64;
        assert!(format_relative_age(now - 5).ends_with("s ago"));
        assert!(format_relative_age(now - 600).ends_with("m ago"));
        assert!(format_relative_age(now - 3600 * 3).ends_with("h ago"));
        assert!(format_relative_age(now - 86400 * 2).ends_with("d ago"));
    }

    /// `human_bytes` produces a one-decimal value with the right unit. Bytes
    /// under 1024 stay raw (no decimal) so "512 B" reads naturally.
    #[test]
    fn human_bytes_picks_unit_and_format() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1024), "1.0 KiB");
        assert_eq!(human_bytes(1024 * 1024 * 5), "5.0 MiB");
        assert_eq!(human_bytes(2u64 * 1024 * 1024 * 1024), "2.0 GiB");
    }

    /// `truncate` adds an ellipsis when over the limit and is a no-op otherwise.
    /// Used to keep the LAST RESULT cell from overflowing the table column.
    #[test]
    fn truncate_short_string_unchanged() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_string_ends_with_ellipsis() {
        let out = truncate("abcdefghijklmnop", 5);
        assert_eq!(out.chars().count(), 5);
        assert!(out.ends_with('…'));
    }
}
