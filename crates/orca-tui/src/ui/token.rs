//! The Token view: cluster-token rotation progress (#265).

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Row, Table, Wrap};

use crate::state::AppState;
use crate::token_actions::node_state;

pub fn draw_token(f: &mut Frame, area: Rect, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(7), Constraint::Min(4)])
        .split(area);

    let summary: Vec<Line> = match &state.rotation {
        None => vec![Line::from("  Loading rotation status…")],
        Some(r) if !r.in_progress => vec![
            Line::from("  No rotation in progress."),
            Line::from(""),
            Line::from("  s starts one: the master writes a new token to"),
            Line::from(format!(
                "  {} and keeps accepting the old one until you finish.",
                r.token_file
            )),
        ],
        Some(r) => {
            let done = r.nodes.iter().filter(|n| n.rotated && n.persisted).count();
            vec![
                Line::from(format!(
                    "  Rotation in progress: {done} of {} agents on the new token for good.",
                    r.nodes.len()
                )),
                Line::from(""),
                Line::from("  Update CI, laptops and scripts that use the cluster token, then"),
                Line::from(
                    "  f retires the old one (refused while an agent isn't done; F forces).",
                ),
            ]
        }
    };
    let block = Block::default()
        .title(" Cluster token ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    f.render_widget(
        Paragraph::new(summary)
            .block(block)
            .wrap(Wrap { trim: false }),
        chunks[0],
    );

    let nodes = state
        .rotation
        .as_ref()
        .map(|r| r.nodes.as_slice())
        .unwrap_or_default();
    let rows: Vec<Row> = nodes
        .iter()
        .map(|n| {
            let color = match (n.rotated, n.persisted) {
                (true, true) => Color::Green,
                (true, false) => Color::Yellow,
                _ => Color::DarkGray,
            };
            Row::new(vec![
                n.node_id.to_string(),
                n.address.clone(),
                node_state(n.rotated, n.persisted).to_string(),
                n.detail.clone().unwrap_or_default(),
            ])
            .style(Style::default().fg(color))
        })
        .collect();
    let header = Row::new(vec!["NODE", "ADDRESS", "STATE", "DETAIL"]).style(
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );
    let widths = [
        Constraint::Length(20),
        Constraint::Length(24),
        Constraint::Length(40),
        Constraint::Min(20),
    ];
    let block = Block::default()
        .title(format!(" Agents ({}) ", nodes.len()))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    f.render_widget(
        Table::new(rows, widths).header(header).block(block),
        chunks[1],
    );
}
