//! Secrets organizer view — keys grouped by inferred scope, with reference
//! count per key. `Enter` drills into the per-key reference list.
//!
//! "Inferred scope":
//! - Stored keys with no template references → `global` (orphan / cluster-wide).
//! - Stored keys referenced by services in exactly one project → that project.
//! - Stored keys referenced by services in multiple projects → `global` (shared).
//! - Keys that appear in env templates but are missing from the store → `broken refs`.
//!
//! Project-scoped secrets (`${secrets.<scope>.KEY}`) get their explicit scope
//! shown verbatim — they don't go through the inference path.

use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders, Paragraph, Row, Table};

use crate::api::SecretUsage;
use crate::state::AppState;

pub fn draw_secrets(f: &mut Frame, area: Rect, state: &AppState) {
    if state.secrets_usage.is_empty() {
        let block = Block::default()
            .title(" Secrets (0) ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));
        let para = Paragraph::new("  Loading... (press 'r' to refresh) or no secrets configured.")
            .block(block);
        f.render_widget(para, area);
        return;
    }

    let rows_data = flatten(&state.secrets_usage);
    let total_keys = rows_data
        .iter()
        .filter(|r| matches!(r, FlatRow::Key { .. }))
        .count();

    // Window the flat list so the selection cursor stays on screen. Without
    // this the Table widget renders top-of-list only — moving the cursor
    // past the bottom of the visible area appears to "freeze" it because
    // the highlight is being drawn off-screen.
    //
    // Reserve 3 rows for the surrounding Block borders + header.
    let visible_rows = (area.height as usize).saturating_sub(3).max(1);
    let scroll = compute_scroll(state.selected_secret, visible_rows, rows_data.len());
    let end = (scroll + visible_rows).min(rows_data.len());

    let rows: Vec<Row> = rows_data[scroll..end]
        .iter()
        .enumerate()
        .map(|(offset, r)| {
            let actual = scroll + offset;
            let selected = actual == state.selected_secret;
            render_row(r, selected)
        })
        .collect();

    let widths = [Constraint::Min(40), Constraint::Length(14)];

    let title = if rows_data.len() > visible_rows {
        format!(
            " Secrets ({total_keys}) [{}/{}] ",
            (scroll + 1).min(rows_data.len()),
            rows_data.len()
        )
    } else {
        format!(" Secrets ({total_keys}) ")
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let header =
        Row::new(vec!["KEY", "REFERENCES"]).style(Style::default().add_modifier(Modifier::BOLD));
    let table = Table::new(rows, widths).header(header).block(block);
    f.render_widget(table, area);
}

/// Keep `selected` inside the visible window of `visible` rows. Mirrors the
/// services view's scroll logic in `ui/table.rs` so cursor movement feels
/// consistent across the two list views.
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

/// One render slot in the flat list. Group headers and key rows interleave so
/// the selection index can walk the whole list naturally; key-only navigation
/// is `selectable_indices(...)`.
pub enum FlatRow<'a> {
    Group { label: String, count: usize },
    Key { usage: &'a SecretUsage },
}

pub fn flatten(usage: &[SecretUsage]) -> Vec<FlatRow<'_>> {
    use std::collections::BTreeMap;

    // Bucket each usage into a group. The infer_group rule lives in one place
    // so the test suite can pin the contract.
    let mut groups: BTreeMap<String, Vec<&SecretUsage>> = BTreeMap::new();
    for u in usage {
        groups.entry(infer_group(u)).or_default().push(u);
    }

    let mut out: Vec<FlatRow<'_>> = Vec::new();
    // Stable ordering: `global` first (most common), then `broken refs`,
    // then project groups alphabetically.
    let mut group_order: Vec<&String> = groups.keys().collect();
    group_order.sort_by_key(|s| (sort_rank(s), (*s).clone()));
    for name in group_order {
        let entries = &groups[name];
        out.push(FlatRow::Group {
            label: name.clone(),
            count: entries.len(),
        });
        for u in entries {
            out.push(FlatRow::Key { usage: u });
        }
    }
    out
}

/// Selection should skip group headers — return the flat indices that
/// correspond to navigable rows (key entries only).
pub fn selectable_indices(rows: &[FlatRow]) -> Vec<usize> {
    rows.iter()
        .enumerate()
        .filter_map(|(i, r)| matches!(r, FlatRow::Key { .. }).then_some(i))
        .collect()
}

fn infer_group(u: &SecretUsage) -> String {
    if !u.in_store {
        return "broken refs".to_string();
    }
    if let Some(scope) = &u.scope {
        return scope.clone();
    }
    let projects: std::collections::BTreeSet<&str> =
        u.refs.iter().filter_map(|r| r.project.as_deref()).collect();
    match projects.len() {
        0 => "global".to_string(),
        1 => projects.iter().next().unwrap().to_string(),
        _ => "global".to_string(),
    }
}

fn sort_rank(name: &str) -> u8 {
    match name {
        "global" => 0,
        "broken refs" => 2,
        _ => 1,
    }
}

fn render_row<'a>(r: &FlatRow<'a>, selected: bool) -> Row<'a> {
    match r {
        FlatRow::Group { label, count } => {
            let style = Style::default()
                .fg(group_color(label))
                .add_modifier(Modifier::BOLD);
            Row::new(vec![
                Span::styled(format!("▾ {label}"), style),
                Span::styled(
                    format!("{count} key(s)"),
                    Style::default().fg(Color::DarkGray),
                ),
            ])
        }
        FlatRow::Key { usage } => {
            let count = usage.refs.len();
            let count_text = match count {
                0 => "no refs".to_string(),
                1 => "1 ref".to_string(),
                n => format!("{n} refs"),
            };
            let count_color = if !usage.in_store {
                Color::Red
            } else if count == 0 {
                Color::Yellow
            } else {
                Color::Green
            };
            let row_style = if selected {
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            Row::new(vec![
                Span::raw(format!("    {}", usage.key)),
                Span::styled(count_text, Style::default().fg(count_color)),
            ])
            .style(row_style)
        }
    }
}

fn group_color(name: &str) -> Color {
    match name {
        "global" => Color::Cyan,
        "broken refs" => Color::Red,
        _ => Color::Yellow,
    }
}

/// Pure mapping from selected flat-index to the underlying `SecretUsage`.
/// Used by the event-loop drill-down handler so it can stay decoupled from
/// the rendering structure.
pub fn selected_key(state: &AppState) -> Option<&SecretUsage> {
    let rows = flatten(&state.secrets_usage);
    match rows.get(state.selected_secret) {
        Some(FlatRow::Key { usage }) => Some(usage),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::SecretRef;

    fn usage(key: &str, in_store: bool, refs: &[(&str, Option<&str>)]) -> SecretUsage {
        SecretUsage {
            key: key.into(),
            scope: None,
            refs: refs
                .iter()
                .map(|(s, p)| SecretRef {
                    service_name: (*s).into(),
                    project: p.map(String::from),
                })
                .collect(),
            in_store,
        }
    }

    /// An orphan (stored, no refs) shows up under `global` — that's where
    /// cluster-level secrets live by convention.
    #[test]
    fn orphan_groups_under_global() {
        assert_eq!(infer_group(&usage("X", true, &[])), "global");
    }

    /// Single-project refs → that project becomes the group. This is the
    /// most common case in practice.
    #[test]
    fn single_project_refs_groups_under_project() {
        let u = usage("DB_URL", true, &[("api", Some("backend"))]);
        assert_eq!(infer_group(&u), "backend");
    }

    /// Cross-project usage demotes to `global` — the user can't safely put
    /// the secret in just one project once multiple projects depend on it.
    #[test]
    fn multi_project_refs_group_under_global() {
        let u = usage(
            "STRIPE",
            true,
            &[("api", Some("backend")), ("dash", Some("frontend"))],
        );
        assert_eq!(infer_group(&u), "global");
    }

    /// Refs from services not in any project (e.g. cluster-level workers)
    /// also count as global.
    #[test]
    fn unprojected_refs_group_under_global() {
        let u = usage("X", true, &[("daemon", None)]);
        assert_eq!(infer_group(&u), "global");
    }

    /// A template references a key that isn't stored — surfaces as broken.
    #[test]
    fn missing_store_entry_groups_under_broken_refs() {
        let u = usage("GONE", false, &[("api", Some("backend"))]);
        assert_eq!(infer_group(&u), "broken refs");
    }

    /// Headers come first; key rows follow inside each group. Tests that
    /// global outranks projects which outrank broken refs.
    #[test]
    fn flatten_orders_groups_global_projects_broken() {
        let usages = vec![
            usage("A", true, &[("svc", Some("alpha"))]),
            usage("B", true, &[]),
            usage("C", false, &[("svc", None)]),
        ];
        let rows = flatten(&usages);
        // Sequence: global header, B, alpha header, A, broken refs header, C
        match &rows[0] {
            FlatRow::Group { label, .. } => assert_eq!(label, "global"),
            _ => panic!("expected group header first"),
        }
        match &rows[2] {
            FlatRow::Group { label, .. } => assert_eq!(label, "alpha"),
            _ => panic!("expected alpha group"),
        }
        match &rows[4] {
            FlatRow::Group { label, .. } => assert_eq!(label, "broken refs"),
            _ => panic!("expected broken refs"),
        }
    }

    /// Selection navigation must skip group headers — indices returned point
    /// only at `Key` rows, in order.
    #[test]
    fn selectable_indices_skip_group_headers() {
        let usages = vec![
            usage("A", true, &[("svc", Some("alpha"))]),
            usage("B", true, &[]),
        ];
        let rows = flatten(&usages);
        let sel = selectable_indices(&rows);
        for i in &sel {
            assert!(matches!(rows[*i], FlatRow::Key { .. }));
        }
        assert_eq!(sel.len(), 2);
    }
}
