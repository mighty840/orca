//! Pure line-building for the networks tree. No drawing here — keeping this
//! free of `Frame`/`Rect` is what makes the topology rendering unit-testable.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::api::{NodeNetworks, NodeRole};

pub(super) fn render_node_ascii(node: &NodeNetworks, out: &mut Vec<Line>) {
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
        for (i, d) in node.domains.iter().enumerate() {
            let is_last = i == node.domains.len() - 1;
            let prefix = if is_last { "└─ " } else { "├─ " };

            let mut spans = vec![
                Span::raw("    "),
                Span::styled(prefix, Style::default().fg(Color::DarkGray)),
                Span::styled(d.domain.clone(), Style::default().fg(Color::White)),
                Span::styled(" → ", Style::default().fg(Color::DarkGray)),
                Span::raw(d.service.clone()),
            ];
            // Show the resolved A-record next to the domain. `?` when DNS
            // didn't answer within the dashboard timeout — operators read
            // that as "I should look at why this name doesn't resolve."
            let (ip, ip_color) = match &d.resolved_ip {
                Some(ip) => (ip.clone(), Color::DarkGray),
                None => ("?".to_string(), Color::Yellow),
            };
            spans.push(Span::styled(
                format!("  [A: {ip}]"),
                Style::default().fg(ip_color),
            ));
            out.push(Line::from(spans));
        }
    }

    // Bridge networks.
    if node.networks.is_empty() {
        out.push(dim_line("    (no orca-* bridge networks on this node)"));
        return;
    }
    out.push(section_line("Docker networks"));
    for (net_idx, net) in node.networks.iter().enumerate() {
        let is_last_net = net_idx == node.networks.len() - 1;
        let net_prefix = if is_last_net { "└─ " } else { "├─ " };
        // Continuation column under this network's connector: a vertical
        // line while sibling networks follow below, blank after the last.
        let net_cont = if is_last_net { "    " } else { "│   " };

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
            out.push(dim_line(&format!("    {net_cont}(no containers attached)")));
            continue;
        }

        for (svc_idx, svc) in net.services.iter().enumerate() {
            let is_last_svc = svc_idx == net.services.len() - 1;
            let svc_branch = if is_last_svc {
                "└── "
            } else {
                "├── "
            };
            let svc_cont = if is_last_svc { "    " } else { "│   " };

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
                Span::raw("    "),
                Span::styled(
                    format!("{net_cont}{svc_branch}"),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::raw(svc.name.clone()),
                Span::styled("  aliases: ", Style::default().fg(Color::DarkGray)),
                Span::styled(aliases, Style::default().fg(alias_color)),
            ]));

            // Missing-alias warning row. Drawn red so it pops in the green
            // sea of aliases — this is the row that means "something
            // references me by a name that won't resolve."
            if !svc.missing_aliases.is_empty() {
                let names = svc.missing_aliases.join(", ");
                out.push(Line::from(vec![
                    Span::raw("    "),
                    Span::styled(
                        format!("{net_cont}{svc_cont}"),
                        Style::default().fg(Color::DarkGray),
                    ),
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

pub(super) fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}
