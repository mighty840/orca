//! Webhooks view — list every registered webhook with last-invocation summary.

use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders, Row, Table};

use super::backups::format_relative_age;
use crate::api::WebhookEntry;
use crate::state::AppState;

pub fn draw_webhooks(f: &mut Frame, area: Rect, state: &AppState) {
    if state.webhooks.is_empty() {
        let block = Block::default()
            .title(" Webhooks (0) ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));
        let text = "  No webhooks registered. Press 'a' to add one.";
        f.render_widget(ratatui::widgets::Paragraph::new(text).block(block), area);
        return;
    }

    let header = Row::new(vec![
        "REPO", "BRANCH", "SERVICE", "TYPE", "SECRET", "LAST", "STATUS", "COMMIT",
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));

    let rows: Vec<Row> = state
        .webhooks
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let selected = i == state.selected_webhook;
            let style = if selected {
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            Row::new(webhook_row(w)).style(style)
        })
        .collect();

    let widths = [
        Constraint::Min(20),
        Constraint::Length(12),
        Constraint::Length(20),
        Constraint::Length(7),
        Constraint::Length(7),
        Constraint::Length(12),
        Constraint::Length(7),
        Constraint::Length(10),
    ];

    let block = Block::default()
        .title(format!(" Webhooks ({}) ", state.webhooks.len()))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let table = Table::new(rows, widths).header(header).block(block);
    f.render_widget(table, area);
}

fn webhook_row(w: &WebhookEntry) -> Vec<Span<'static>> {
    let kind = if w.infra { "infra" } else { "deploy" };
    let secret = if w.has_secret { "✓" } else { "—" };

    let (last_age, status_cell, commit) = match &w.last_invocation {
        Some(inv) => {
            let age = format_relative_age(inv.at.timestamp() as u64);
            let status = inv.status_code.to_string();
            let commit = inv.commit_sha.chars().take(8).collect::<String>();
            (age, status, commit)
        }
        None => ("never".into(), "—".into(), "—".into()),
    };

    let status_color = match &w.last_invocation {
        Some(inv) if inv.deployed => Color::Green,
        Some(inv) if (400..500).contains(&inv.status_code) => Color::Yellow,
        Some(_) => Color::Red,
        None => Color::Gray,
    };

    vec![
        Span::raw(w.repo.clone()),
        Span::raw(w.branch.clone()),
        Span::raw(w.service_name.clone()),
        Span::raw(kind.to_string()),
        Span::raw(secret.to_string()),
        Span::raw(last_age),
        Span::styled(status_cell, Style::default().fg(status_color)),
        Span::raw(commit),
    ]
}
