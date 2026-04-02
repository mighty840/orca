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
        .title(format!(" Orca v{VERSION} -- Help "))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let ks = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let ds = Style::default().fg(Color::White);
    let hs = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(Color::DarkGray);

    let mut lines: Vec<Line> = Vec::new();

    // Navigation
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("  Navigation", hs)));
    for (k, d) in [
        ("j / Down", "Move selection down"),
        ("k / Up", "Move selection up"),
        ("Enter", "Open service detail"),
        ("Esc", "Back / clear filter"),
        ("g / G", "Jump to top / bottom"),
    ] {
        lines.push(bind(k, d, ks, ds));
    }

    // Views
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("  Views", hs)));
    for (k, d) in [
        ("1", "Services view"),
        ("2 / n", "Nodes view"),
        ("3 / m", "Metrics view"),
        ("l", "Logs for selected service"),
        ("?", "This help screen"),
    ] {
        lines.push(bind(k, d, ks, ds));
    }

    // Actions
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("  Actions", hs)));
    for (k, d) in [
        ("s", "Scale service (opens :scale prompt)"),
        ("x", "Stop selected service"),
        ("p", "Filter by project of selected"),
        ("r", "Refresh immediately"),
        ("/", "Filter services by name"),
        ("w", "Toggle word wrap (in logs)"),
    ] {
        lines.push(bind(k, d, ks, ds));
    }

    // Commands
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("  Commands (:)", hs)));
    for (k, d) in [
        (":scale <svc> <n>", "Scale service to n replicas"),
        (":stop <svc>", "Stop a service"),
        (":stop-project <p>", "Stop entire project"),
        (":deploy", "Info on redeploying"),
        (":filter <text>", "Filter services"),
        (":project <name>", "Filter by project"),
        (":metrics", "Metrics view"),
        (":drain <id>", "Drain a node"),
        (":undrain <id>", "Undrain a node"),
        (":exec <svc> <cmd>", "Info on exec"),
        (":q", "Quit"),
    ] {
        lines.push(bind(k, d, ks, ds));
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

fn bind<'a>(key: &'a str, desc: &'a str, key_style: Style, desc_style: Style) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!("    {key:<20}"), key_style),
        Span::styled(desc, desc_style),
    ])
}
