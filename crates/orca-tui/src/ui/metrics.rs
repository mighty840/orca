//! Metrics view — formatted display of Prometheus metrics and resource usage.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Row, Table};

use crate::state::AppState;

/// Draw the metrics view with cluster overview and per-service resources.
pub fn draw_metrics(f: &mut Frame, area: Rect, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(8), // cluster summary
            Constraint::Min(5),    // per-service resources
        ])
        .split(area);

    draw_summary(f, chunks[0], state);
    draw_resource_table(f, chunks[1], state);
}

fn draw_summary(f: &mut Frame, area: Rect, state: &AppState) {
    let (running, stopped, degraded) = state.status_counts();
    let total = state.services.len();
    let heading = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let label = Style::default().fg(Color::DarkGray);
    let val = Style::default().fg(Color::White);
    let green = Style::default().fg(Color::Green);
    let red = Style::default().fg(Color::Red);
    let yellow = Style::default().fg(Color::Yellow);

    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled("  Cluster Overview", heading)),
        Line::from(vec![
            Span::styled("  Total services:   ", label),
            Span::styled(total.to_string(), val),
        ]),
        Line::from(vec![
            Span::styled("  Running:          ", label),
            Span::styled(running.to_string(), green),
        ]),
        Line::from(vec![
            Span::styled("  Stopped/Failed:   ", label),
            Span::styled(stopped.to_string(), if stopped > 0 { red } else { val }),
        ]),
    ];
    if degraded > 0 {
        lines.push(Line::from(vec![
            Span::styled("  Degraded:         ", label),
            Span::styled(degraded.to_string(), yellow),
        ]));
    }
    lines.push(Line::from(vec![
        Span::styled("  Nodes:            ", label),
        Span::styled(state.node_count.to_string(), val),
    ]));

    let block = Block::default()
        .title(" Cluster Summary ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_resource_table(f: &mut Frame, area: Rect, state: &AppState) {
    let rows: Vec<Row> = state
        .services
        .iter()
        .map(|svc| {
            let mem = svc.memory_usage.as_deref().unwrap_or("-");
            let cpu = svc
                .cpu_percent
                .map(|c| format!("{c:.1}%"))
                .unwrap_or_else(|| "-".into());
            let project = svc.project.as_deref().unwrap_or("-");
            let status_color = super::status_color(&svc.status);

            Row::new(vec![
                svc.name.clone(),
                project.to_string(),
                svc.status.clone(),
                format!("{}/{}", svc.running_replicas, svc.desired_replicas),
                mem.to_string(),
                cpu,
            ])
            .style(Style::default().fg(status_color))
        })
        .collect();

    let header = Row::new(vec![
        "NAME", "PROJECT", "STATUS", "REPLICAS", "MEMORY", "CPU",
    ])
    .style(
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );

    let widths = [
        Constraint::Min(18),
        Constraint::Min(14),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Length(8),
    ];

    let block = Block::default()
        .title(format!(" Resource Usage ({}) ", state.services.len()))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let table = Table::new(rows, widths).header(header).block(block);
    f.render_widget(table, area);
}
