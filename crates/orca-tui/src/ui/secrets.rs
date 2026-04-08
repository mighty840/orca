//! Secrets management view.
//!
//! Lists secret keys (values are never returned by the API). Supports
//! `:set KEY VALUE` and `:rm KEY` via the command bar already wired up
//! through the existing `commands.rs` dispatch.

use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Row, Table};

use crate::state::AppState;

pub fn draw_secrets(f: &mut Frame, area: Rect, state: &AppState) {
    let total = state.secret_keys.len();
    let title = format!(" Secrets ({total}) ");

    let visible = if area.height > 4 {
        (area.height - 4) as usize
    } else {
        1
    };
    let scroll = if state.selected_secret >= visible {
        state.selected_secret + 1 - visible
    } else {
        0
    };
    let end = (scroll + visible).min(total);

    let rows: Vec<Row> = state.secret_keys[scroll..end]
        .iter()
        .enumerate()
        .map(|(vi, key)| {
            let actual = scroll + vi;
            let sel = actual == state.selected_secret;
            let style = if sel {
                Style::default()
                    .bg(Color::DarkGray)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Yellow)
            };
            let pointer = if sel { ">" } else { " " };
            // Values are never sent over the wire — show a static glyph
            // so an over-the-shoulder peek can't reveal anything sensitive.
            Row::new(vec![format!("{pointer} {key}"), "********".to_string()]).style(style)
        })
        .collect();

    let header = Row::new(vec!["  KEY", "VALUE"])
        .style(
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .bottom_margin(0);

    let widths = [Constraint::Min(30), Constraint::Min(12)];

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let table = Table::new(rows, widths).header(header).block(block);
    f.render_widget(table, area);
}
