//! Drawing and scroll windowing for the networks view. The tree content
//! itself is built in `render.rs`.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};

use super::render::{plural, render_node_ascii};
use crate::api::ClusterNetworksResponse;
use crate::state::AppState;

fn build_lines(resp: &ClusterNetworksResponse) -> Vec<Line<'static>> {
    let mut lines: Vec<Line> = Vec::new();
    for node in &resp.nodes {
        render_node_ascii(node, &mut lines);
        lines.push(Line::from(""));
    }
    lines
}

/// Total rendered lines for the current networks data. Used by `keys.rs::G`
/// to snap the scroll viewport to the last screen without having to peek at
/// the frame area at key-handling time.
pub fn rendered_line_count(state: &AppState) -> usize {
    let Some(resp) = &state.networks else {
        return 0;
    };
    build_lines(resp).len()
}

pub fn draw_networks(f: &mut Frame, area: Rect, state: &AppState) {
    let Some(resp) = &state.networks else {
        let block = Block::default()
            .title(" Networks ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));
        let para = Paragraph::new("  Loading cluster network topology... (press 'r' to refresh)")
            .block(block);
        f.render_widget(para, area);
        return;
    };

    let total_nodes = resp.nodes.len();
    let total_bridges: usize = resp.nodes.iter().map(|n| n.networks.len()).sum();
    let total_domains: usize = resp.nodes.iter().map(|n| n.domains.len()).sum();

    let lines = build_lines(resp);

    // Window the tree to keep `state.network_scroll` rows at the top. The
    // view has no selection cursor — scroll is purely viewport offset that
    // j/k/PgUp/PgDn move. 2 rows reserved for the top/bottom border of the
    // surrounding Block.
    let visible_rows = (area.height as usize).saturating_sub(2).max(1);
    let max_scroll = lines.len().saturating_sub(visible_rows);
    let scroll = state.network_scroll.min(max_scroll);
    let end = (scroll + visible_rows).min(lines.len());
    let view: Vec<Line> = lines[scroll..end].to_vec();

    let scroll_indicator = if lines.len() > visible_rows {
        format!(" [{}/{}] ", scroll + 1, lines.len())
    } else {
        String::new()
    };
    let title = format!(
        " Networks ({total_nodes} node{}, {total_bridges} bridge{}, {total_domains} domain{}){scroll_indicator}",
        plural(total_nodes),
        plural(total_bridges),
        plural(total_domains),
    );
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    f.render_widget(Paragraph::new(view).block(block), area);
}
