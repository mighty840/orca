//! Alerts view — list + drill-down for AI alert conversations.

use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Row, Table, Wrap};

use crate::api::{AlertSender, AlertSeverity, AlertState};
use crate::state::AppState;

/// Top-level alerts table.
pub fn draw_alerts(f: &mut Frame, area: Rect, state: &AppState) {
    if state.alerts_unavailable {
        let block = Block::default()
            .title(" Alerts (unavailable) ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow));
        let text = "  AI alerts are not configured.\n\n  Add an [ai] block + [ai.alerts.channels.*] to cluster.toml, then restart the server.";
        f.render_widget(Paragraph::new(text).block(block), area);
        return;
    }
    if state.alerts.is_empty() {
        let scope = if state.alerts_show_all {
            "all"
        } else {
            "active"
        };
        let block = Block::default()
            .title(format!(" Alerts ({scope}: 0) "))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));
        let text = "  No conversations.\n\n  Press 'a' to toggle showing dismissed/resolved.";
        f.render_widget(Paragraph::new(text).block(block), area);
        return;
    }

    let header = Row::new(vec![
        "SEVERITY", "SERVICE", "STATE", "AGE", "TURNS", "LATEST",
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));

    let rows: Vec<Row> = state
        .alerts
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let selected = i == state.selected_alert;
            let base = if selected {
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let sev = severity_label(a.severity);
            let sev_style = base.fg(severity_color(a.severity));
            let state_style = base.fg(state_color(a.state));
            let latest = a
                .messages
                .last()
                .map(|m| truncate(&m.content.replace('\n', " "), 60))
                .unwrap_or_default();
            let age = format_age(a.started_at);
            Row::new(vec![
                Span::styled(sev.to_string(), sev_style),
                Span::styled(a.service.clone(), base),
                Span::styled(format!("{:?}", a.state).to_lowercase(), state_style),
                Span::styled(age, base),
                Span::styled(a.messages.len().to_string(), base),
                Span::styled(latest, base),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(10),
        Constraint::Min(14),
        Constraint::Length(15),
        Constraint::Length(8),
        Constraint::Length(6),
        Constraint::Min(30),
    ];

    let scope = if state.alerts_show_all {
        "all"
    } else {
        "active"
    };
    let critical = state
        .alerts
        .iter()
        .filter(|a| a.severity == AlertSeverity::Critical)
        .count();
    let title = if critical > 0 {
        format!(
            " Alerts ({scope}: {}, {critical} critical) ",
            state.alerts.len()
        )
    } else {
        format!(" Alerts ({scope}: {}) ", state.alerts.len())
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let table = Table::new(rows, widths).header(header).block(block);
    f.render_widget(table, area);
}

/// Drill-down view for one conversation.
pub fn draw_alert_detail(f: &mut Frame, area: Rect, state: &AppState, id: &str) {
    let Some(conv) = state.alerts.iter().find(|a| a.id.to_string() == id) else {
        let block = Block::default()
            .title(" Alert Detail (not found) ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow));
        f.render_widget(
            Paragraph::new("  Alert not in the current list. Press Esc to go back.").block(block),
            area,
        );
        return;
    };

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(vec![
        Span::styled("Service: ", Style::default().add_modifier(Modifier::BOLD)),
        Span::raw(conv.service.clone()),
        Span::raw("  "),
        Span::styled("Severity: ", Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(
            severity_label(conv.severity),
            Style::default().fg(severity_color(conv.severity)),
        ),
        Span::raw("  "),
        Span::styled("State: ", Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(
            format!("{:?}", conv.state).to_lowercase(),
            Style::default().fg(state_color(conv.state)),
        ),
    ]));
    lines.push(Line::from(format!(
        "Started: {}   ID: {}",
        conv.started_at.to_rfc3339(),
        conv.id
    )));
    lines.push(Line::from(""));

    for msg in &conv.messages {
        let (label, color) = match msg.sender {
            AlertSender::Orca => ("orca", Color::Cyan),
            AlertSender::Operator => ("you", Color::Green),
            AlertSender::System => ("system", Color::DarkGray),
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!("[{}] ", msg.timestamp.format("%H:%M:%S")),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                format!("{label}: "),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
        ]));
        for content_line in msg.content.lines() {
            lines.push(Line::from(format!("  {content_line}")));
        }
        if let Some(cmd) = &msg.suggested_command {
            lines.push(Line::from(vec![
                Span::styled("    fix: ", Style::default().fg(Color::Yellow)),
                Span::styled(cmd.clone(), Style::default().fg(Color::Yellow)),
            ]));
        }
        lines.push(Line::from(""));
    }

    let total = lines.len();
    let block = Block::default()
        .title(format!(
            " Alert: {} — {} [{}/{}] ",
            conv.service,
            severity_label(conv.severity),
            state.alert_detail_scroll + 1,
            total.max(1)
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let para = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((state.alert_detail_scroll as u16, 0));
    f.render_widget(para, area);
}

fn severity_label(s: AlertSeverity) -> &'static str {
    match s {
        AlertSeverity::Critical => "CRITICAL",
        AlertSeverity::Warning => "WARNING",
        AlertSeverity::Info => "INFO",
    }
}

fn severity_color(s: AlertSeverity) -> Color {
    match s {
        AlertSeverity::Critical => Color::Red,
        AlertSeverity::Warning => Color::Yellow,
        AlertSeverity::Info => Color::Cyan,
    }
}

fn state_color(s: AlertState) -> Color {
    match s {
        AlertState::Investigating => Color::Yellow,
        AlertState::AwaitingAction => Color::Magenta,
        AlertState::Acknowledged => Color::Cyan,
        AlertState::Remediated | AlertState::Resolved => Color::Green,
        AlertState::Dismissed => Color::DarkGray,
    }
}

fn format_age(started: chrono::DateTime<chrono::Utc>) -> String {
    let secs = (chrono::Utc::now() - started).num_seconds().max(0);
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
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

/// Resolve the alert ID the current view targets: drill-down uses its path
/// id; list view uses the selected row. `None` outside those views or when
/// the list is empty. Used by both keymap (d / R / Enter) and command-mode
/// handlers (`:reply`, `:dismiss`, `:resolve`).
pub fn current_alert_id(state: &AppState) -> Option<String> {
    use crate::state::View;
    match &state.view {
        View::AlertDetail { id } => Some(id.clone()),
        View::Alerts => state
            .alerts
            .get(state.selected_alert)
            .map(|a| a.id.to_string()),
        _ => None,
    }
}

/// Count of critical alerts currently active — used by the header badge.
pub fn critical_count(state: &AppState) -> usize {
    state
        .alerts
        .iter()
        .filter(|a| {
            a.severity == AlertSeverity::Critical
                && !matches!(
                    a.state,
                    AlertState::Resolved | AlertState::Dismissed | AlertState::Remediated
                )
        })
        .count()
}
