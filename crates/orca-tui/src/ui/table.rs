//! Full-width service table (k9s style) — replaces the old services panel.

use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Row, Table};

use crate::state::AppState;

use super::{status_color, status_icon};

/// Draw the full-width service table with scroll support.
pub fn draw_table(f: &mut Frame, area: Rect, state: &AppState) {
    let filtered = state.filtered_services();
    let title = build_title(state, filtered.len());

    // Calculate visible area (subtract 3 for borders + header row)
    let visible_rows = if area.height > 4 {
        (area.height - 4) as usize
    } else {
        1
    };

    let scroll = compute_scroll(state.selected_service, visible_rows, filtered.len());
    let end = (scroll + visible_rows).min(filtered.len());

    let rows: Vec<Row> = filtered[scroll..end]
        .iter()
        .enumerate()
        .map(|(vi, svc)| {
            let actual_idx = scroll + vi;
            let sel = actual_idx == state.selected_service;
            let icon = status_icon(&svc.status);
            let s_color = status_color(&svc.status);
            let domain = svc.domain.as_deref().unwrap_or("-");
            let project = svc.project.as_deref().unwrap_or("-");

            let style = if sel {
                Style::default()
                    .bg(Color::DarkGray)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(s_color)
            };

            let pointer = if sel { ">" } else { " " };

            Row::new(vec![
                format!("{pointer} {icon} {}", svc.name),
                project.to_string(),
                svc.image.clone(),
                svc.runtime.clone(),
                format!("{}/{}", svc.running_replicas, svc.desired_replicas),
                svc.status.clone(),
                domain.to_string(),
            ])
            .style(style)
        })
        .collect();

    let header = Row::new(vec![
        "  NAME", "PROJECT", "IMAGE", "RUNTIME", "REPLICAS", "STATUS", "DOMAIN",
    ])
    .style(
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    )
    .bottom_margin(0);

    let widths = [
        Constraint::Min(18),
        Constraint::Min(12),
        Constraint::Min(18),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Min(14),
    ];

    let scroll_indicator = if filtered.len() > visible_rows {
        format!(
            " Services ({}) [{}-{}/{}] ",
            filtered.len(),
            scroll + 1,
            end,
            filtered.len()
        )
    } else {
        title
    };

    let block = Block::default()
        .title(scroll_indicator)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let table = Table::new(rows, widths).header(header).block(block);
    f.render_widget(table, area);
}

fn build_title(state: &AppState, count: usize) -> String {
    let mut parts = Vec::new();
    if !state.filter.is_empty() {
        parts.push(format!("filter:{}", state.filter));
    }
    if let Some(ref proj) = state.project_filter {
        parts.push(format!("project:{proj}"));
    }
    if parts.is_empty() {
        format!(" Services ({count}) ")
    } else {
        format!(" Services [{}] ({count}) ", parts.join(" "))
    }
}

/// Compute the scroll offset to keep `selected` visible within `visible` rows.
fn compute_scroll(selected: usize, visible: usize, total: usize) -> usize {
    if total <= visible {
        return 0;
    }
    if selected < visible / 2 {
        return 0;
    }
    let ideal = selected.saturating_sub(visible / 2);
    ideal.min(total.saturating_sub(visible))
}
