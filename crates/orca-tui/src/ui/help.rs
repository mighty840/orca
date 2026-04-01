//! Help overlay popup with grouped keybindings.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::state::AppState;

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Draw a centered help overlay with grouped keybindings.
pub fn draw_help(f: &mut Frame, area: Rect, state: &AppState) {
    let popup = centered_rect(55, 70, area);
    f.render_widget(Clear, popup);

    let block = Block::default()
        .title(format!(" Orca v{VERSION} "))
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
        ("Tab", "Cycle panel focus"),
        ("1 / 2 / 3", "Jump to Services / Logs / Nodes"),
        ("Enter", "Open service detail view"),
        ("Esc", "Back / clear filter"),
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
        ("c", "Copy service name"),
        ("/", "Filter services by name"),
    ] {
        lines.push(binding_line(k, d, key_style, desc_style));
    }

    // Views
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("  Views", heading_style)));
    for (k, d) in [
        ("l", "Show logs for selected service"),
        ("n", "Show nodes panel"),
        ("w", "Toggle word wrap in logs"),
        ("?", "Toggle this help overlay"),
        ("q", "Quit"),
    ] {
        lines.push(binding_line(k, d, key_style, desc_style));
    }

    // API URL at bottom
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
    f.render_widget(para, popup);
}

fn binding_line<'a>(key: &'a str, desc: &'a str, key_style: Style, desc_style: Style) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!("    {key:<14}"), key_style),
        Span::styled(desc, desc_style),
    ])
}

/// Build a centered rectangle of `percent_x` x `percent_y` within `area`.
fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vert = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    let horiz = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vert[1]);
    horiz[1]
}
