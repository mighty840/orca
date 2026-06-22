//! Full-width service table (k9s style) — replaces the old services panel.
//!
//! Rows are grouped by `project`. Each project is a collapsible header row;
//! pressing space on a service row collapses or expands the parent project.
//! Services without a project fall under the synthetic group `(no project)`.

use std::collections::BTreeMap;

use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, Borders, Row, Table};

use crate::api::ServiceStatus;
use crate::state::AppState;

use super::{status_color, status_icon};

const NO_PROJECT: &str = "(no project)";

/// One row of the rendered services table — either a project header or a
/// child service. Selection only ever points at service rows.
enum DisplayRow<'a> {
    ProjectHeader { name: &'a str, count: usize },
    Service(&'a ServiceStatus),
}

/// Draw the full-width service table with project grouping + scroll.
pub fn draw_table(f: &mut Frame, area: Rect, state: &AppState) {
    let filtered = state.filtered_services();
    let display = build_display_rows(&filtered, state);
    let title = build_title(state, filtered.len());

    // `selected_service` already indexes into `visible_services()` (the
    // same ordering `build_display_rows` produces). Map it to the index
    // inside the interleaved `display` vec by finding the Nth service row.
    let selected_pos = display
        .iter()
        .enumerate()
        .filter(|(_, r)| matches!(r, DisplayRow::Service(_)))
        .nth(state.selected_service)
        .map(|(i, _)| i)
        .unwrap_or(0);

    let visible_rows = if area.height > 4 {
        (area.height - 4) as usize
    } else {
        1
    };
    let scroll = compute_scroll(selected_pos, visible_rows, display.len());
    let end = (scroll + visible_rows).min(display.len());

    let rows: Vec<Row> = display[scroll..end]
        .iter()
        .enumerate()
        .map(|(vi, row)| {
            let actual = scroll + vi;
            match row {
                DisplayRow::ProjectHeader { name, count } => {
                    let collapsed = state.collapsed_projects.contains(*name);
                    let glyph = if collapsed { "▶" } else { "▼" };
                    Row::new(vec![
                        format!("  {glyph} {name}"),
                        format!("{count} services"),
                        String::new(),
                        String::new(),
                        String::new(),
                        String::new(),
                        String::new(),
                        String::new(),
                    ])
                    .style(
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    )
                }
                DisplayRow::Service(svc) => {
                    let sel = actual == selected_pos;
                    let icon = status_icon(&svc.status);
                    let s_color = status_color(&svc.status);
                    // Show the primary domain, with a "(+N)" hint when the
                    // service is served on multiple hostnames (apex+www, etc.).
                    let domain = if svc.domains.len() > 1 {
                        format!("{} (+{})", svc.domains[0], svc.domains.len() - 1)
                    } else {
                        svc.domains
                            .first()
                            .or(svc.domain.as_ref())
                            .cloned()
                            .unwrap_or_else(|| "-".to_string())
                    };
                    let project = svc.project.as_deref().unwrap_or("-");
                    let node = svc.node.as_deref().unwrap_or("master");
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
                        format!("{pointer}  {icon} {}", svc.name),
                        project.to_string(),
                        svc.image.clone(),
                        svc.runtime.clone(),
                        format!("{}/{}", svc.running_replicas, svc.desired_replicas),
                        svc.status.clone(),
                        node.to_string(),
                        domain.to_string(),
                    ])
                    .style(style)
                }
            }
        })
        .collect();

    let header = Row::new(vec![
        "  NAME", "PROJECT", "IMAGE", "RUNTIME", "REPLICAS", "STATUS", "NODE", "DOMAIN",
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
        Constraint::Length(12),
        Constraint::Min(14),
    ];

    let scroll_indicator = if display.len() > visible_rows {
        format!(
            " Services ({}) [{}-{}/{}] ",
            filtered.len(),
            scroll + 1,
            end,
            display.len()
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

/// Build the interleaved (project header, service row, project header, ...)
/// display list. Services in collapsed projects are dropped here.
fn build_display_rows<'a>(filtered: &[&'a ServiceStatus], state: &AppState) -> Vec<DisplayRow<'a>> {
    // Stable group order: alphabetical by project name. Services keep their
    // original order within a group so the table doesn't reshuffle.
    let mut grouped: BTreeMap<&'a str, Vec<&'a ServiceStatus>> = BTreeMap::new();
    for svc in filtered {
        let key = svc.project.as_deref().unwrap_or(NO_PROJECT);
        grouped.entry(key).or_default().push(*svc);
    }

    let mut out: Vec<DisplayRow<'a>> = Vec::new();
    for (project, svcs) in grouped {
        out.push(DisplayRow::ProjectHeader {
            name: project,
            count: svcs.len(),
        });
        if state.collapsed_projects.contains(project) {
            continue;
        }
        for s in svcs {
            out.push(DisplayRow::Service(s));
        }
    }
    out
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
