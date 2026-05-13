//! Webhook invocation history drill-down — last N pushes for one webhook.

use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders, Paragraph, Row, Table};

use crate::state::AppState;

pub fn draw_webhook_invocations(f: &mut Frame, area: Rect, state: &AppState, service: &str) {
    if state.webhook_invocations.is_empty() {
        let block = Block::default()
            .title(format!(" Invocations — {service} (0) "))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));
        let text = "  No invocations yet. They appear here as pushes arrive.";
        f.render_widget(Paragraph::new(text).block(block), area);
        return;
    }

    let header = Row::new(vec![
        "WHEN", "STATUS", "DEPLOYED", "REPO", "BRANCH", "COMMIT",
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));

    let rows: Vec<Row> = state
        .webhook_invocations
        .iter()
        .rev()
        .map(|inv| {
            let status_color = if inv.deployed {
                Color::Green
            } else if (400..500).contains(&inv.status_code) {
                Color::Yellow
            } else {
                Color::Red
            };
            Row::new(vec![
                Span::raw(inv.at.format("%Y-%m-%d %H:%M:%S UTC").to_string()),
                Span::styled(
                    inv.status_code.to_string(),
                    Style::default().fg(status_color),
                ),
                Span::raw(if inv.deployed { "yes" } else { "no" }.to_string()),
                Span::raw(inv.repo.clone()),
                Span::raw(inv.branch.clone()),
                Span::raw(inv.commit_sha.chars().take(8).collect::<String>()),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(22),
        Constraint::Length(7),
        Constraint::Length(9),
        Constraint::Min(20),
        Constraint::Length(12),
        Constraint::Length(10),
    ];

    let block = Block::default()
        .title(format!(
            " Invocations — {service} ({}) ",
            state.webhook_invocations.len()
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let table = Table::new(rows, widths).header(header).block(block);
    f.render_widget(table, area);
}
