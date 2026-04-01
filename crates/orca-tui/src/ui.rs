//! TUI rendering — top-level layout and header/footer.

mod detail;
mod help;
mod logs;
mod nodes;
mod services;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::state::{AppState, ConnectionStatus, InputMode, Panel};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Render the full dashboard.
pub fn draw(f: &mut Frame, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // header / status bar
            Constraint::Min(10),   // main
            Constraint::Length(1), // footer
        ])
        .split(f.area());

    draw_header(f, chunks[0], state);

    if state.panel == Panel::Detail {
        detail::draw_detail(f, chunks[1], state);
    } else {
        draw_main(f, chunks[1], state);
    }

    draw_footer(f, chunks[2], state);

    if state.show_help {
        help::draw_help(f, f.area(), state);
    }
}

fn draw_header(f: &mut Frame, area: Rect, state: &AppState) {
    let (running, stopped, degraded) = state.status_counts();
    let total = state.services.len();

    // Blinking connection dot — blink every ~5 ticks (~500ms)
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
        Span::styled(format!("v{VERSION} "), Style::default().fg(Color::DarkGray)),
        Span::styled(dot, Style::default().fg(dot_color)),
        Span::styled(" | ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            state.cluster_name.clone(),
            Style::default().fg(Color::White),
        ),
        Span::styled(" | ", Style::default().fg(Color::DarkGray)),
        Span::raw("Svc: "),
        svc_summary,
        Span::styled(" | ", Style::default().fg(Color::DarkGray)),
        Span::raw("Nodes: "),
        Span::styled(
            state.node_count.to_string(),
            Style::default().fg(Color::Cyan),
        ),
        Span::styled(" | ", Style::default().fg(Color::DarkGray)),
        Span::raw("Up: "),
        Span::styled(state.uptime_str(), Style::default().fg(Color::Green)),
    ]);
    let block = Block::default().borders(Borders::BOTTOM);
    let para = Paragraph::new(text).block(block);
    f.render_widget(para, area);
}

fn draw_main(f: &mut Frame, area: Rect, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(area);

    services::draw_services(f, chunks[0], state);

    match state.panel {
        Panel::Services | Panel::Logs => logs::draw_logs(f, chunks[1], state),
        Panel::Nodes => nodes::draw_nodes(f, chunks[1], state),
        Panel::Detail => {} // handled above
    }
}

fn draw_footer(f: &mut Frame, area: Rect, state: &AppState) {
    let text = if let Some(err) = &state.error {
        Line::from(Span::styled(err.clone(), Style::default().fg(Color::Red)))
    } else if state.input_mode == InputMode::Filter {
        Line::from(vec![
            Span::styled(" /", Style::default().fg(Color::Yellow)),
            Span::raw(state.filter.clone()),
            Span::styled("_", Style::default().fg(Color::Yellow)),
            Span::raw("  (Esc to clear)"),
        ])
    } else if let Some(msg) = &state.status_msg {
        Line::from(Span::styled(
            format!(" {msg}"),
            Style::default().fg(Color::Green),
        ))
    } else if !state.filter.is_empty() {
        Line::from(vec![
            Span::styled(" filter: ", Style::default().fg(Color::Yellow)),
            Span::raw(state.filter.clone()),
            Span::raw("  "),
            footer_keys(),
        ])
    } else {
        Line::from(footer_keys())
    };
    f.render_widget(Paragraph::new(text), area);
}

fn footer_keys() -> Span<'static> {
    Span::styled(
        " j/k:nav  Tab:panel  1/2/3:jump  /:filter  Enter:detail  d:deploy  x:stop  w:wrap  r:refresh  ?:help  q:quit",
        Style::default().fg(Color::DarkGray),
    )
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
pub fn status_icon(status: &str) -> (&'static str, Color) {
    match status {
        "running" => ("\u{25cf}", Color::Green),          // ●
        "degraded" => ("\u{25d0}", Color::Yellow),        // ◐
        "stopped" | "failed" => ("\u{25cb}", Color::Red), // ○
        "creating" | "starting" => ("\u{25d0}", Color::Blue),
        _ => ("\u{25cb}", Color::Gray),
    }
}
