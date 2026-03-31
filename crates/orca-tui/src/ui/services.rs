//! Service list panel rendering.

use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Row, Table};

use crate::state::{AppState, Panel};

use super::status_color;

/// Draw the service list as a table with column headers.
pub fn draw_services(f: &mut Frame, area: Rect, state: &AppState) {
    let border_style = if state.panel == Panel::Services {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let filtered = state.filtered_services();
    let title = if state.filter.is_empty() {
        format!(" Services ({}) ", filtered.len())
    } else {
        format!(" Services [{}] ({}) ", state.filter, filtered.len())
    };

    let rows: Vec<Row> = filtered
        .iter()
        .enumerate()
        .map(|(i, svc)| {
            let sel = i == state.selected_service;
            let s_color = status_color(&svc.status);
            let domain = svc.domain.as_deref().unwrap_or("-");
            let marker = if sel { ">" } else { " " };

            let base_style = if sel {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(s_color)
            };

            Row::new(vec![
                format!("{marker} {}", svc.name),
                svc.runtime.clone(),
                format!("{}/{}", svc.running_replicas, svc.desired_replicas),
                svc.status.clone(),
                domain.to_string(),
            ])
            .style(base_style)
        })
        .collect();

    let header = Row::new(vec!["  NAME", "RUNTIME", "REPLICAS", "STATUS", "DOMAIN"])
        .style(
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .bottom_margin(0);

    let widths = [
        Constraint::Min(16),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Min(12),
    ];

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(border_style);

    let table = Table::new(rows, widths).header(header).block(block);
    f.render_widget(table, area);
}
