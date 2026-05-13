//! Cluster networks view — three-layer tree:
//!   1. Public edge (domains) per node, with the service each domain routes to.
//!   2. Docker bridge networks (`orca-*`) per node, with attached services
//!      and their network aliases.
//!   3. Unreachable agents are listed with a placeholder so the operator
//!      sees who didn't respond.
//!
//! The original issue (#17) calls for an ASCII routing graph rendered with
//! ratatui's canvas widget. This view delivers the data acceptance criteria
//! (per-node, per-bridge, services + aliases) as a tree-list; the graph
//! rendering is a follow-up.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::api::{NodeNetworks, NodeRole};
use crate::state::AppState;

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

    let mut lines: Vec<Line> = Vec::new();
    for node in &resp.nodes {
        render_node(node, &mut lines);
        lines.push(Line::from(""));
    }

    let title = format!(
        " Networks ({total_nodes} node{}, {total_bridges} bridge{}, {total_domains} domain{}) ",
        plural(total_nodes),
        plural(total_bridges),
        plural(total_domains),
    );
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_node(node: &NodeNetworks, out: &mut Vec<Line>) {
    let role = match node.role {
        NodeRole::Master => "master",
        NodeRole::Agent => "agent",
    };
    let header_color = match node.role {
        NodeRole::Master => Color::Cyan,
        NodeRole::Agent => Color::Yellow,
    };
    let header = format!("▾ {} ({role})", node.hostname);
    out.push(Line::from(vec![Span::styled(
        header,
        Style::default()
            .fg(header_color)
            .add_modifier(Modifier::BOLD),
    )]));

    if !node.reachable {
        out.push(dim_line(
            "    (agent unreachable — last cached state unavailable)",
        ));
        return;
    }

    // Public edge section.
    if !node.domains.is_empty() {
        out.push(section_line("Public edge"));
        for d in &node.domains {
            out.push(Line::from(vec![
                Span::raw("    "),
                Span::styled(d.domain.clone(), Style::default().fg(Color::White)),
                Span::styled(" → ", Style::default().fg(Color::DarkGray)),
                Span::raw(d.service.clone()),
            ]));
        }
    }

    // Bridge networks.
    if node.networks.is_empty() {
        out.push(dim_line("    (no orca-* bridge networks on this node)"));
        return;
    }
    out.push(section_line("Docker networks"));
    for net in &node.networks {
        let net_header = format!(
            "    {} ({} service{})",
            net.name,
            net.services.len(),
            plural(net.services.len())
        );
        out.push(Line::from(vec![Span::styled(
            net_header,
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        )]));
        if net.services.is_empty() {
            out.push(dim_line("        (no containers attached)"));
            continue;
        }
        for svc in &net.services {
            let aliases = if svc.aliases.is_empty() {
                "(no aliases)".to_string()
            } else {
                svc.aliases.join(", ")
            };
            let alias_color = if svc.aliases.is_empty() {
                Color::Yellow
            } else {
                Color::Green
            };
            out.push(Line::from(vec![
                Span::raw("        "),
                Span::raw(svc.name.clone()),
                Span::styled("  aliases: ", Style::default().fg(Color::DarkGray)),
                Span::styled(aliases, Style::default().fg(alias_color)),
            ]));
        }
    }
}

fn section_line(label: &str) -> Line<'static> {
    Line::from(vec![Span::styled(
        format!("  {label}"),
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    )])
}

fn dim_line(text: &str) -> Line<'static> {
    Line::from(vec![Span::styled(
        text.to_string(),
        Style::default().fg(Color::DarkGray),
    )])
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `plural` is what keeps "1 node" from rendering as "1 nodes". Trivial
    /// but easy to silently break by inverting the comparison.
    #[test]
    fn plural_is_empty_for_one() {
        assert_eq!(plural(0), "s");
        assert_eq!(plural(1), "");
        assert_eq!(plural(2), "s");
        assert_eq!(plural(99), "s");
    }
}
