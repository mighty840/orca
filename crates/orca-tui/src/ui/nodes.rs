//! Nodes panel rendering.

use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Paragraph, Row, Table};

use crate::state::AppState;

/// Draw the nodes table.
pub fn draw_nodes(f: &mut Frame, area: Rect, state: &AppState) {
    let border_style = Style::default().fg(Color::Cyan);
    let block = Block::default()
        .title(format!(" Nodes ({}) ", state.nodes.len()))
        .borders(Borders::ALL)
        .border_style(border_style);

    if state.nodes.is_empty() {
        let para = Paragraph::new("No nodes registered (single-node mode)").block(block);
        f.render_widget(para, area);
        return;
    }

    let rows: Vec<Row> = state
        .nodes
        .iter()
        .map(|n| {
            Row::new(vec![
                n.node_id.to_string(),
                n.address.clone(),
                n.last_heartbeat.chars().take(19).collect::<String>(),
            ])
        })
        .collect();

    let header = Row::new(vec!["ID", "ADDRESS", "LAST HEARTBEAT"]).style(
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );
    let widths = [
        Constraint::Length(20),
        Constraint::Length(25),
        Constraint::Min(20),
    ];
    let table = Table::new(rows, widths).header(header).block(block);
    f.render_widget(table, area);
}
