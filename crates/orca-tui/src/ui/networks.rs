//! Cluster networks view — three-layer tree:
//!   1. Public edge (domains) per node, with the service each domain routes to.
//!   2. Docker bridge networks (`orca-*`) per node, with attached services
//!      and their network aliases.
//!   3. Unreachable agents are listed with a placeholder so the operator
//!      sees who didn't respond.
//!
//! Renders the cluster topology as ASCII art with hierarchical indentation.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::api::{NodeNetworks, NodeRole};
use crate::state::AppState;

/// Total rendered lines for the current networks data. Used by `keys.rs::G`
/// to snap the scroll viewport to the last screen without having to peek at
/// the frame area at key-handling time.
pub fn rendered_line_count(state: &AppState) -> usize {
    let Some(resp) = &state.networks else {
        return 0;
    };
    let mut lines: Vec<Line> = Vec::new();
    for node in &resp.nodes {
        render_node_ascii(node, &mut lines);
        lines.push(Line::from(""));
    }
    lines.len()
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

    let mut lines: Vec<Line> = Vec::new();
    for node in &resp.nodes {
        render_node_ascii(node, &mut lines);
        lines.push(Line::from(""));
    }

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

fn render_node_ascii(node: &NodeNetworks, out: &mut Vec<Line>) {
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

    // Public edge section - render as ASCII art
    if !node.domains.is_empty() {
        out.push(section_line("Public edge"));
        for (i, d) in node.domains.iter().enumerate() {
            let is_last = i == node.domains.len() - 1;
            let prefix = if is_last { "└─ " } else { "├─ " };

            let spans = vec![
                Span::raw("    "),
                Span::styled(prefix.to_string(), Style::default().fg(Color::DarkGray)),
                Span::styled(d.domain.clone(), Style::default().fg(Color::White)),
                Span::styled("  [A: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    d.resolved_ip.as_ref().map_or("?".to_string(), Clone::clone),
                    Style::default().fg(if d.resolved_ip.is_some() {
                        Color::DarkGray
                    } else {
                        Color::Yellow
                    }),
                ),
                Span::styled("]", Style::default().fg(Color::DarkGray)),
            ];

            out.push(Line::from(spans));
        }
    }

    // Bridge networks - render as ASCII art
    if node.networks.is_empty() {
        out.push(dim_line("    (no orca-* bridge networks on this node)"));
        return;
    }
    out.push(section_line("Docker networks"));
    for (net_idx, net) in node.networks.iter().enumerate() {
        let is_last_net = net_idx == node.networks.len() - 1;
        let net_prefix = if is_last_net { "└─ " } else { "├─ " };

        let net_header = format!(
            "    {}{} ({} service{})",
            net_prefix,
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

        // Render services with proper indentation
        for (svc_idx, svc) in net.services.iter().enumerate() {
            let is_last_svc = svc_idx == net.services.len() - 1;
            let svc_prefix = if is_last_net {
                if is_last_svc { "    " } else { "│   " }
            } else {
                if is_last_svc {
                    "└── "
                } else {
                    "├── "
                }
            };

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

            let mut line_parts = vec![
                Span::raw("        "),
                Span::styled(svc_prefix.to_string(), Style::default().fg(Color::DarkGray)),
                Span::raw(svc.name.clone()),
            ];

            // Add aliases
            if !svc.aliases.is_empty() {
                line_parts.push(Span::styled(
                    "  aliases: ",
                    Style::default().fg(Color::DarkGray),
                ));
                line_parts.push(Span::styled(aliases, Style::default().fg(alias_color)));
            }
            out.push(Line::from(line_parts));

            // Missing-alias warning row
            if !svc.missing_aliases.is_empty() {
                let names = svc.missing_aliases.join(", ");
                let warning_prefix = if is_last_svc {
                    "        "
                } else {
                    "│       "
                };

                out.push(Line::from(vec![
                    Span::raw("        "),
                    Span::styled(warning_prefix, Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        "⚠ referenced as: ",
                        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(names, Style::default().fg(Color::Red)),
                    Span::styled(
                        "  (add to network aliases)",
                        Style::default().fg(Color::DarkGray),
                    ),
                ]));
            }
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
