//! Full-screen service detail view (k9s style).

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::state::AppState;

use super::status_color;

/// Draw full-screen detail view for the given service name.
pub fn draw_detail(f: &mut Frame, area: Rect, state: &AppState, service_name: &str) {
    let svc = state.services.iter().find(|s| s.name == service_name);

    let svc = match svc {
        Some(s) => s,
        None => {
            let block = Block::default()
                .title(format!(" Detail: {service_name} "))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan));
            let para = Paragraph::new("Service not found").block(block);
            f.render_widget(para, area);
            return;
        }
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(14), // service info
            Constraint::Min(5),     // recent logs
            Constraint::Length(1),  // action bar
        ])
        .split(area);

    draw_info(f, chunks[0], svc);
    draw_recent_logs(f, chunks[1], state);
    draw_actions(f, chunks[2]);
}

fn draw_info(f: &mut Frame, area: Rect, svc: &crate::api::ServiceStatus) {
    let s_color = status_color(&svc.status);
    let domain = svc.domain.as_deref().unwrap_or("-");
    let project = svc.project.as_deref().unwrap_or("-");
    let memory = svc.memory_usage.as_deref().unwrap_or("-");
    let cpu = svc
        .cpu_percent
        .map(|c| format!("{c:.1}%"))
        .unwrap_or_else(|| "-".into());
    let label = Style::default().fg(Color::DarkGray);
    let val = Style::default().fg(Color::White);
    let accent = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);

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

    let info_lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::styled("  Name:      ", label),
            Span::styled(svc.name.clone(), accent),
        ]),
        Line::from(vec![
            Span::styled("  Project:   ", label),
            Span::styled(project.to_string(), val),
        ]),
        Line::from(vec![
            Span::styled("  Image:     ", label),
            Span::styled(svc.image.clone(), val),
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
            Span::styled("  Health:    ", label),
            Span::styled(health_str, Style::default().fg(health_color)),
        ]),
        Line::from(vec![
            Span::styled("  Domain:    ", label),
            Span::styled(domain.to_string(), Style::default().fg(Color::Blue)),
        ]),
        Line::from(vec![
            Span::styled("  Memory:    ", label),
            Span::styled(memory.to_string(), val),
            Span::styled("  CPU: ", label),
            Span::styled(cpu, val),
        ]),
    ];

    let block = Block::default()
        .title(format!(" {} ", svc.name))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let info = Paragraph::new(info_lines).block(block);
    f.render_widget(info, area);
}

fn draw_recent_logs(f: &mut Frame, area: Rect, state: &AppState) {
    let log_lines: Vec<&str> = state.logs.lines().collect();
    let inner_h = if area.height > 2 {
        (area.height - 2) as usize
    } else {
        0
    };
    let start = log_lines.len().saturating_sub(inner_h);

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

    let wrap_hint = if state.word_wrap { " [wrap] " } else { "" };
    let log_block = Block::default()
        .title(format!(" Recent Logs{wrap_hint}"))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let mut log_para = Paragraph::new(recent).block(log_block);
    if state.word_wrap {
        log_para = log_para.wrap(Wrap { trim: false });
    }
    f.render_widget(log_para, area);
}

fn draw_actions(f: &mut Frame, area: Rect) {
    let key = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let desc = Style::default().fg(Color::DarkGray);

    let bar = Line::from(vec![
        Span::styled(" [s]", key),
        Span::styled("cale ", desc),
        Span::styled("[x]", key),
        Span::styled("stop ", desc),
        Span::styled("[l]", key),
        Span::styled("ogs ", desc),
        Span::styled("[Esc]", key),
        Span::styled("back", desc),
    ]);
    f.render_widget(Paragraph::new(bar), area);
}
