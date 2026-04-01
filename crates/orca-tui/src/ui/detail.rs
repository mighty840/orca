//! Service detail view — shown when pressing Enter on a service.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::state::AppState;

use super::status_color;

/// Draw the service detail view.
pub fn draw_detail(f: &mut Frame, area: Rect, state: &AppState) {
    let svc = match state.selected_service_data() {
        Some(s) => s,
        None => {
            let block = Block::default()
                .title(" Detail ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan));
            let para = Paragraph::new("No service selected").block(block);
            f.render_widget(para, area);
            return;
        }
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(10), Constraint::Min(5)])
        .split(area);

    // --- Info section ---
    let s_color = status_color(&svc.status);
    let domain = svc.domain.as_deref().unwrap_or("-");

    let health_str = if svc.running_replicas == svc.desired_replicas {
        "healthy"
    } else {
        "unhealthy"
    };
    let health_color = if health_str == "healthy" {
        Color::Green
    } else {
        Color::Red
    };

    let label = Style::default().fg(Color::DarkGray);
    let val = Style::default().fg(Color::White);
    let accent = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);

    let info_lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  Name:      ", label),
            Span::styled(svc.name.clone(), accent),
        ]),
        Line::from(vec![
            Span::styled("  Runtime:   ", label),
            Span::styled(svc.runtime.clone(), val),
        ]),
        Line::from(vec![
            Span::styled("  Replicas:  ", label),
            Span::styled(
                format!("{}/{}", svc.running_replicas, svc.desired_replicas),
                val,
            ),
        ]),
        Line::from(vec![
            Span::styled("  Status:    ", label),
            Span::styled(svc.status.clone(), Style::default().fg(s_color)),
        ]),
        Line::from(vec![
            Span::styled("  Domain:    ", label),
            Span::styled(domain.to_string(), Style::default().fg(Color::Blue)),
        ]),
        Line::from(vec![
            Span::styled("  Health:    ", label),
            Span::styled(health_str, Style::default().fg(health_color)),
        ]),
    ];

    let info_block = Block::default()
        .title(format!(" {} ", svc.name))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let info = Paragraph::new(info_lines).block(info_block);
    f.render_widget(info, chunks[0]);

    // --- Recent logs section ---
    let log_lines: Vec<&str> = state.logs.lines().collect();
    let tail_n = 20;
    let start = if log_lines.len() > tail_n {
        log_lines.len() - tail_n
    } else {
        0
    };
    let recent: Vec<Line> = log_lines[start..]
        .iter()
        .enumerate()
        .map(|(i, line)| {
            Line::from(vec![
                Span::styled(
                    format!("{:>3} ", start + i + 1),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::raw((*line).to_string()),
            ])
        })
        .collect();

    let log_block = Block::default()
        .title(" Recent Logs (Esc to go back) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    let log_para = Paragraph::new(recent).block(log_block);
    f.render_widget(log_para, chunks[1]);
}
