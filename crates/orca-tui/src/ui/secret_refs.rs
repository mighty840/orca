//! Per-secret reference list — services that template this key in their env.

use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Paragraph, Row, Table};

use crate::state::AppState;

pub fn draw_secret_refs(f: &mut Frame, area: Rect, state: &AppState, key: &str) {
    let usage = state.secrets_usage.iter().find(|u| u.key == key);
    let Some(usage) = usage else {
        let block = Block::default()
            .title(format!(" Refs — {key} "))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow));
        let para = Paragraph::new(
            "  Secret no longer in the cached response. Press Esc to go back, 'r' to refresh.",
        )
        .block(block);
        f.render_widget(para, area);
        return;
    };

    if usage.refs.is_empty() {
        let block = Block::default()
            .title(format!(" Refs — {key} (0) "))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));
        let msg = if usage.in_store {
            "  No services reference this secret. Safe to delete if it's not used elsewhere."
        } else {
            "  Key is referenced but missing from the store — fix with `:set <KEY> <value>`."
        };
        f.render_widget(Paragraph::new(msg).block(block), area);
        return;
    }

    let header =
        Row::new(vec!["SERVICE", "PROJECT"]).style(Style::default().add_modifier(Modifier::BOLD));
    let rows: Vec<Row> = usage
        .refs
        .iter()
        .map(|r| {
            Row::new(vec![
                r.service_name.clone(),
                r.project.clone().unwrap_or_else(|| "(none)".into()),
            ])
        })
        .collect();

    let widths = [Constraint::Min(30), Constraint::Min(20)];
    let block = Block::default()
        .title(format!(
            " Refs — {key} ({} service{}) ",
            usage.refs.len(),
            if usage.refs.len() == 1 { "" } else { "s" }
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let table = Table::new(rows, widths).header(header).block(block);
    f.render_widget(table, area);
}
