//! Full-screen help view with grouped keybindings (k9s style).

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::state::AppState;

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Draw full-screen help view with grouped keybindings.
pub fn draw_help(f: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default()
        .title(format!(" Orca v{VERSION} — Help "))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let key_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let desc_style = Style::default().fg(Color::White);
    let heading_style = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(Color::DarkGray);

    let mut lines: Vec<Line> = Vec::new();

    // Navigation
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("  Navigation", heading_style)));
    for (k, d) in [
        ("j / Down", "Move selection down"),
        ("k / Up", "Move selection up"),
        ("Enter", "Open service detail"),
        ("Esc", "Go back to previous view"),
        ("g", "Jump to top"),
        ("G", "Jump to bottom"),
    ] {
        lines.push(binding_line(k, d, key_style, desc_style));
    }

    // Views
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("  Views", heading_style)));
    for (k, d) in [
        ("l", "Full-screen logs for selected service"),
        ("?", "This help screen"),
        (":services", "Service list (default view)"),
        (":nodes", "Node list"),
        (":logs <svc>", "Full-screen logs for <svc>"),
        (":q / :quit", "Quit"),
    ] {
        lines.push(binding_line(k, d, key_style, desc_style));
    }

    // Actions
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("  Actions", heading_style)));
    for (k, d) in [
        ("d", "Deploy (redeploy) service"),
        ("x", "Stop service"),
        ("s", "Show scale info"),
        ("r", "Refresh immediately"),
        ("/", "Filter services by name"),
        ("w", "Toggle word wrap (in logs)"),
        (":", "Enter command mode"),
    ] {
        lines.push(binding_line(k, d, key_style, desc_style));
    }

    // API info
    lines.push(Line::from(""));
    let api_display = if state.api_url.is_empty() {
        "not connected"
    } else {
        &state.api_url
    };
    lines.push(Line::from(vec![
        Span::styled("  API: ", dim),
        Span::styled(api_display.to_string(), dim),
    ]));

    let para = Paragraph::new(lines).block(block);
    f.render_widget(para, area);
}

fn binding_line<'a>(key: &'a str, desc: &'a str, key_style: Style, desc_style: Style) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!("    {key:<16}"), key_style),
        Span::styled(desc, desc_style),
    ])
}
