//! Service list panel rendering with scrolling and status icons.

use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Row, Table};

use crate::state::{AppState, Panel};

use super::{status_color, status_icon};

/// Extract image tag (part after last ':') or "latest" if none.
fn image_tag(image: &str) -> &str {
    if image.is_empty() {
        return "-";
    }
    // Handle digest references (sha256:...)
    if image.contains("@sha256:") {
        return "sha256";
    }
    image
        .rsplit_once(':')
        .map(|(_, tag)| tag)
        .unwrap_or("latest")
}

/// Draw the service list as a table with column headers and scrolling.
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

    // Calculate visible area (subtract 3 for borders + header row)
    let visible_rows = if area.height > 4 {
        (area.height - 4) as usize
    } else {
        1
    };

    // Compute scroll offset to keep selection visible
    let scroll = compute_scroll(state.selected_service, visible_rows, filtered.len());

    let end = (scroll + visible_rows).min(filtered.len());

    let rows: Vec<Row> = filtered[scroll..end]
        .iter()
        .enumerate()
        .map(|(vi, svc)| {
            let actual_idx = scroll + vi;
            let sel = actual_idx == state.selected_service;
            let (icon, icon_color) = status_icon(&svc.status);
            let s_color = status_color(&svc.status);
            let domain = svc.domain.as_deref().unwrap_or("-");
            let tag = image_tag(&svc.image);

            let base_style = if sel {
                Style::default()
                    .bg(Color::DarkGray)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(s_color)
            };

            let icon_style = if sel {
                Style::default()
                    .bg(Color::DarkGray)
                    .fg(icon_color)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(icon_color)
            };

            // We need to use the same style for all cells since Row::style
            // applies to the entire row. Use icon in the name column.
            let _ = icon_style; // icon color embedded in name cell
            Row::new(vec![
                format!("{icon} {}", svc.name),
                svc.runtime.clone(),
                format!("{}/{}", svc.running_replicas, svc.desired_replicas),
                svc.status.clone(),
                tag.to_string(),
                domain.to_string(),
            ])
            .style(base_style)
        })
        .collect();

    let header = Row::new(vec!["  NAME", "RUNTIME", "REPL", "STATUS", "TAG", "DOMAIN"])
        .style(
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .bottom_margin(0);

    let widths = [
        Constraint::Min(16),
        Constraint::Length(10),
        Constraint::Length(6),
        Constraint::Length(10),
        Constraint::Length(12),
        Constraint::Min(10),
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
        .border_style(border_style);

    let table = Table::new(rows, widths).header(header).block(block);
    f.render_widget(table, area);
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
