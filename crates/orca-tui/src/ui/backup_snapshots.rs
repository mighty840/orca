//! Backup snapshots drill-down — list of timestamped snapshots for one node.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Row, Table};

use super::backups::{format_relative_age, human_bytes};
use crate::api::NodeBackupStatus;
use crate::state::AppState;

pub fn draw_backup_snapshots(f: &mut Frame, area: Rect, state: &AppState, node_idx: usize) {
    let Some(node) = state.backups.as_ref().and_then(|b| b.nodes.get(node_idx)) else {
        let block = Block::default()
            .title(" Snapshots ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow));
        let para =
            Paragraph::new("  Node no longer in the latest response — press Esc.").block(block);
        f.render_widget(para, area);
        return;
    };

    if node.snapshots.is_empty() {
        let block = Block::default()
            .title(format!(" Snapshots — {} (0) ", node.hostname))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));
        let para = Paragraph::new(
            "  No snapshots on this node yet. Press 'b' on the Backups view to trigger one.",
        )
        .block(block);
        f.render_widget(para, area);
        return;
    }

    // Snapshot list on top, file detail underneath.
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(6), Constraint::Length(10)])
        .split(area);

    draw_snapshot_table(f, chunks[0], state, node);
    draw_files_detail(f, chunks[1], state, node);
}

fn draw_snapshot_table(f: &mut Frame, area: Rect, state: &AppState, node: &NodeBackupStatus) {
    let header = Row::new(vec!["#", "TIMESTAMP", "AGE", "FILES", "SIZE"])
        .style(Style::default().add_modifier(Modifier::BOLD));

    let rows: Vec<Row> = node
        .snapshots
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let selected = i == state.selected_backup_snapshot;
            let style = if selected {
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let timestamp = chrono::DateTime::<chrono::Utc>::from_timestamp(s.epoch_secs as i64, 0)
                .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or_else(|| s.epoch_secs.to_string());
            Row::new(vec![
                Span::raw(format!("{i:>3}")),
                Span::raw(timestamp),
                Span::raw(format_relative_age(s.epoch_secs)),
                Span::raw(s.files.len().to_string()),
                Span::raw(human_bytes(s.total_size_bytes)),
            ])
            .style(style)
        })
        .collect();

    let widths = [
        Constraint::Length(4),
        Constraint::Length(22),
        Constraint::Length(14),
        Constraint::Length(8),
        Constraint::Length(12),
    ];

    let block = Block::default()
        .title(format!(
            " Snapshots — {} ({}, local only) ",
            node.hostname,
            node.snapshots.len()
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let table = Table::new(rows, widths).header(header).block(block);
    f.render_widget(table, area);
}

fn draw_files_detail(f: &mut Frame, area: Rect, state: &AppState, node: &NodeBackupStatus) {
    let Some(snap) = node.snapshots.get(state.selected_backup_snapshot) else {
        return;
    };

    let mut lines: Vec<Line> = Vec::with_capacity(snap.files.len() + 2);
    lines.push(Line::from(vec![
        Span::styled("Files: ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(format!(
            "{} ({} total)",
            snap.files.len(),
            human_bytes(snap.total_size_bytes),
        )),
    ]));
    if snap.files.is_empty() {
        lines.push(Line::from(Span::styled(
            "  (empty — likely a failed run)",
            Style::default().fg(Color::Gray),
        )));
    } else {
        for f in &snap.files {
            lines.push(Line::from(format!(
                "  {:<40} {}",
                f.name,
                human_bytes(f.size_bytes),
            )));
        }
    }

    let block = Block::default()
        .title(" Files ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    f.render_widget(Paragraph::new(lines).block(block), area);
}
