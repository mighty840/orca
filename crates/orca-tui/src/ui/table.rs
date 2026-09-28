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

    let cols = columns_for(area.width);
    let rows: Vec<Row> = display[scroll..end]
        .iter()
        .enumerate()
        .map(|(vi, row)| {
            let actual = scroll + vi;
            match row {
                DisplayRow::ProjectHeader { name, count } => {
                    let collapsed = state.collapsed_projects.contains(*name);
                    let glyph = if collapsed { "▶" } else { "▼" };
                    // Without a PROJECT column the count joins the name.
                    let label = if cols.contains(&Col::Project) {
                        format!("  {glyph} {name}")
                    } else {
                        format!("  {glyph} {name} ({count})")
                    };
                    Row::new(pick(
                        &cols,
                        [
                            label,
                            format!("{count} services"),
                            String::new(),
                            String::new(),
                            String::new(),
                            String::new(),
                            String::new(),
                            String::new(),
                        ],
                    ))
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
                    Row::new(pick(
                        &cols,
                        [
                            format!("{pointer}  {icon} {}", svc.name),
                            project.to_string(),
                            svc.image.clone(),
                            svc.runtime.clone(),
                            format!("{}/{}", svc.running_replicas, svc.desired_replicas),
                            svc.status.clone(),
                            node.to_string(),
                            domain.to_string(),
                        ],
                    ))
                    .style(style)
                }
            }
        })
        .collect();

    let header = Row::new(cols.iter().map(|c| c.spec().0))
        .style(
            Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .bottom_margin(0);

    let widths: Vec<Constraint> = cols.iter().map(|c| c.spec().1).collect();

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

/// A column of the services table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Col {
    Name,
    Project,
    Image,
    Runtime,
    Replicas,
    Status,
    Node,
    Domain,
}

/// All columns, in display order.
const ALL_COLS: [Col; 8] = [
    Col::Name,
    Col::Project,
    Col::Image,
    Col::Runtime,
    Col::Replicas,
    Col::Status,
    Col::Node,
    Col::Domain,
];

/// Dropped first to last as the terminal narrows. NAME, REPLICAS and STATUS
/// always stay; PROJECT goes early because the group headers show it.
const DROP_ORDER: [Col; 5] = [
    Col::Runtime,
    Col::Project,
    Col::Image,
    Col::Domain,
    Col::Node,
];

impl Col {
    /// Header, width constraint, and the width it needs at least. Spare
    /// width goes to the text columns, twice as much to NAME.
    fn spec(self) -> (&'static str, Constraint, u16) {
        match self {
            Col::Name => ("  NAME", Constraint::Fill(2), 18),
            Col::Project => ("PROJECT", Constraint::Fill(1), 12),
            Col::Image => ("IMAGE", Constraint::Fill(1), 18),
            Col::Runtime => ("RUNTIME", Constraint::Length(10), 10),
            Col::Replicas => ("REPLICAS", Constraint::Length(9), 9),
            Col::Status => ("STATUS", Constraint::Length(10), 10),
            Col::Node => ("NODE", Constraint::Length(12), 12),
            Col::Domain => ("DOMAIN", Constraint::Fill(1), 14),
        }
    }
}

/// The columns that fit `width` (the table's outer width). The full table
/// needed ~113 columns; narrower terminals squeezed every column into
/// unreadable slivers (#265).
pub(crate) fn columns_for(width: u16) -> Vec<Col> {
    let needed = |cols: &[Col]| -> u16 {
        let min: u16 = cols.iter().map(|c| c.spec().2).sum();
        // Borders, plus one space between columns.
        min + 2 + cols.len().saturating_sub(1) as u16
    };
    let mut cols = ALL_COLS.to_vec();
    for drop in DROP_ORDER {
        if needed(&cols) <= width {
            break;
        }
        cols.retain(|c| *c != drop);
    }
    cols
}

/// The cells of `cols`, from all eight in display order.
fn pick(cols: &[Col], cells: [String; 8]) -> Vec<String> {
    ALL_COLS
        .iter()
        .zip(cells)
        .filter(|(c, _)| cols.contains(c))
        .map(|(_, cell)| cell)
        .collect()
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

/// "7", or "3 of 7 matching /api" while a `/` filter is set.
pub(crate) fn filtered_count(shown: usize, total: usize, filter: &str) -> String {
    if filter.is_empty() {
        total.to_string()
    } else {
        format!("{shown} of {total} matching /{filter}")
    }
}

/// The rows of a bordered table with a header in `area` that keep
/// `selected` on screen: (first row, row count). Without it a table draws
/// from the top and the cursor moves off-screen (#264).
pub(crate) fn window(selected: usize, area: ratatui::layout::Rect, total: usize) -> (usize, usize) {
    // Two border rows and the header row.
    let visible = (area.height as usize).saturating_sub(3).max(1);
    (compute_scroll(selected, visible, total), visible)
}

/// Compute the scroll offset to keep `selected` visible within `visible` rows.
pub(crate) fn compute_scroll(selected: usize, visible: usize, total: usize) -> usize {
    if total <= visible {
        return 0;
    }
    if selected < visible / 2 {
        return 0;
    }
    let ideal = selected.saturating_sub(visible / 2);
    ideal.min(total.saturating_sub(visible))
}

#[cfg(test)]
mod column_tests {
    use super::{Col, columns_for};

    #[test]
    fn a_wide_terminal_shows_every_column() {
        assert_eq!(columns_for(112).len(), 8);
    }

    #[test]
    fn columns_drop_in_order_as_it_narrows() {
        use Col::*;
        assert_eq!(
            columns_for(111),
            [Name, Project, Image, Replicas, Status, Node, Domain]
        );
        assert_eq!(
            columns_for(100),
            [Name, Image, Replicas, Status, Node, Domain]
        );
        assert_eq!(columns_for(80), [Name, Replicas, Status, Node, Domain]);
        assert_eq!(columns_for(60), [Name, Replicas, Status, Node]);
    }

    #[test]
    fn the_essentials_stay_on_a_tiny_terminal() {
        assert_eq!(columns_for(20), vec![Col::Name, Col::Replicas, Col::Status]);
    }
}
