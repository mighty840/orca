//! TUI rendering — k9s-style full-screen views with header/footer chrome.

pub mod detail;
pub mod help;
pub mod logs;
pub mod nodes;
pub mod table;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::state::{AppState, ConnectionStatus, InputMode, View};

/// Render the full dashboard — dispatches to the current view.
pub fn draw(f: &mut Frame, state: &AppState) {
    let show_cmd = state.input_mode == InputMode::Command || state.input_mode == InputMode::Filter;
    let constraints = if show_cmd {
        vec![
            Constraint::Length(1), // header
            Constraint::Length(1), // breadcrumb
            Constraint::Length(1), // command input (above content)
            Constraint::Min(5),    // main content
            Constraint::Length(1), // footer
        ]
    } else {
        vec![
            Constraint::Length(1), // header
            Constraint::Length(1), // breadcrumb
            Constraint::Min(5),    // main content
            Constraint::Length(1), // footer
        ]
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(f.area());

    draw_header(f, chunks[0], state);
    draw_breadcrumb(f, chunks[1], state);

    if show_cmd {
        draw_command_bar(f, chunks[2], state);
        let content = chunks[3];
        match &state.view {
            View::Services => table::draw_table(f, content, state),
            View::Nodes => nodes::draw_nodes(f, content, state),
            View::Logs { service } => logs::draw_logs(f, content, state, service),
            View::Detail { service } => detail::draw_detail(f, content, state, service),
            View::Help => help::draw_help(f, content, state),
        }
        draw_footer(f, chunks[4], state);
    } else {
        let content = chunks[2];
        match &state.view {
            View::Services => table::draw_table(f, content, state),
            View::Nodes => nodes::draw_nodes(f, content, state),
            View::Logs { service } => logs::draw_logs(f, content, state, service),
            View::Detail { service } => detail::draw_detail(f, content, state, service),
            View::Help => help::draw_help(f, content, state),
        }
        draw_footer(f, chunks[3], state);
    }
}

fn draw_header(f: &mut Frame, area: Rect, state: &AppState) {
    let (running, stopped, degraded) = state.status_counts();
    let total = state.services.len();

    let blink_on = (state.tick / 5).is_multiple_of(2);
    let (dot, dot_color) = match state.connection {
        ConnectionStatus::Connected => {
            if blink_on {
                ("●", Color::Green)
            } else {
                ("●", Color::DarkGray)
            }
        }
        ConnectionStatus::Disconnected => {
            if blink_on {
                ("●", Color::Red)
            } else {
                ("●", Color::DarkGray)
            }
        }
    };

    let svc_summary = if stopped == 0 && degraded == 0 {
        Span::styled(
            format!("{running}/{total} running"),
            Style::default().fg(Color::Green),
        )
    } else {
        let mut parts = format!("{running} up");
        if degraded > 0 {
            parts.push_str(&format!(", {degraded} degraded"));
        }
        if stopped > 0 {
            parts.push_str(&format!(", {stopped} down"));
        }
        let color = if stopped > 0 {
            Color::Red
        } else {
            Color::Yellow
        };
        Span::styled(parts, Style::default().fg(color))
    };

    let text = Line::from(vec![
        Span::styled(
            " orca ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(dot, Style::default().fg(dot_color)),
        Span::styled(" | ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            state.cluster_name.clone(),
            Style::default().fg(Color::White),
        ),
        Span::styled(" | ", Style::default().fg(Color::DarkGray)),
        svc_summary,
        Span::styled(" | ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{} nodes", state.node_count),
            Style::default().fg(Color::Cyan),
        ),
        Span::styled(" | ", Style::default().fg(Color::DarkGray)),
        Span::styled(state.uptime_str(), Style::default().fg(Color::Green)),
    ]);
    f.render_widget(Paragraph::new(text), area);
}

fn draw_breadcrumb(f: &mut Frame, area: Rect, state: &AppState) {
    let crumb = match &state.view {
        View::Services => "Services".to_string(),
        View::Nodes => "Nodes".to_string(),
        View::Logs { service } => format!("Services > {service} > Logs"),
        View::Detail { service } => format!("Services > {service}"),
        View::Help => "Help".to_string(),
    };
    let line = Line::from(vec![
        Span::styled(" ", Style::default()),
        Span::styled(crumb, Style::default().fg(Color::Yellow)),
    ]);
    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(Color::DarkGray));
    f.render_widget(Paragraph::new(line).block(block), area);
}

/// Command/filter input bar — shown ABOVE the content area.
fn draw_command_bar(f: &mut Frame, area: Rect, state: &AppState) {
    let line = if state.input_mode == InputMode::Command {
        Line::from(vec![
            Span::styled(":", Style::default().fg(Color::Cyan)),
            Span::raw(state.command_input.clone()),
            Span::styled("_", Style::default().fg(Color::Cyan)),
        ])
    } else {
        Line::from(vec![
            Span::styled(" /", Style::default().fg(Color::Yellow)),
            Span::raw(state.filter.clone()),
            Span::styled("_", Style::default().fg(Color::Yellow)),
            Span::styled("  (Esc to clear)", Style::default().fg(Color::DarkGray)),
        ])
    };
    f.render_widget(Paragraph::new(line), area);
}

fn draw_footer(f: &mut Frame, area: Rect, state: &AppState) {
    // Error
    if let Some(err) = &state.error {
        let line = Line::from(Span::styled(
            format!(" {err}"),
            Style::default().fg(Color::Red),
        ));
        f.render_widget(Paragraph::new(line), area);
        return;
    }

    // Flash message
    if let Some(msg) = &state.status_msg {
        let line = Line::from(Span::styled(
            format!(" {msg}"),
            Style::default().fg(Color::Green),
        ));
        f.render_widget(Paragraph::new(line), area);
        return;
    }

    // Context-specific key hints
    let keys = match &state.view {
        View::Services => " :command  /filter  j/k:nav  Enter:detail  l:logs  ?:help  q:quit",
        View::Nodes => " :command  Esc:back  ?:help  q:quit",
        View::Logs { .. } => " Esc:back  w:wrap  ?:help  q:quit",
        View::Detail { .. } => " Esc:back  d:deploy  s:scale  x:stop  l:logs  ?:help  q:quit",
        View::Help => " Esc:back  q:quit",
    };
    let line = Line::from(Span::styled(keys, Style::default().fg(Color::DarkGray)));
    f.render_widget(Paragraph::new(line), area);
}

/// Color for a service status string.
pub fn status_color(status: &str) -> Color {
    match status {
        "running" => Color::Green,
        "degraded" => Color::Yellow,
        "stopped" | "failed" => Color::Red,
        "creating" | "starting" => Color::Blue,
        _ => Color::Gray,
    }
}

/// Status indicator character for service status.
pub fn status_icon(status: &str) -> &'static str {
    match status {
        "running" => "\u{25cf}",               // ●
        "degraded" => "\u{25d0}",              // ◐
        "stopped" | "failed" => "\u{25cb}",    // ○
        "creating" | "starting" => "\u{25d0}", // ◐
        _ => "\u{25cb}",                       // ○
    }
}
