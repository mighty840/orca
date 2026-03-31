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

use crate::state::{AppState, InputMode, Panel};

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
        help::draw_help(f, f.area());
    }
}

fn draw_header(f: &mut Frame, area: Rect, state: &AppState) {
    let running = state
        .services
        .iter()
        .filter(|s| s.status == "running")
        .count();
    let total = state.services.len();
    let svc_color = if running == total {
        Color::Green
    } else {
        Color::Yellow
    };
    let text = Line::from(vec![
        Span::styled(
            " orca ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("| ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            state.cluster_name.clone(),
            Style::default().fg(Color::White),
        ),
        Span::styled(" | ", Style::default().fg(Color::DarkGray)),
        Span::raw("Svc: "),
        Span::styled(format!("{running}/{total}"), Style::default().fg(svc_color)),
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
        " j/k:nav  Tab:panel  1/2/3:jump  /:filter  Enter:detail  d:deploy  x:stop  r:refresh  ?:help  q:quit",
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
