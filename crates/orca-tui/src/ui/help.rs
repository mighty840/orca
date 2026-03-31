//! Help overlay popup.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

/// Draw a centered help overlay.
pub fn draw_help(f: &mut Frame, area: Rect) {
    let popup = centered_rect(50, 60, area);
    f.render_widget(Clear, popup);

    let block = Block::default()
        .title(" Keybindings ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let key_style = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let desc_style = Style::default().fg(Color::White);

    let bindings = vec![
        ("j / Down", "Move selection down"),
        ("k / Up", "Move selection up"),
        ("Tab", "Cycle panel focus"),
        ("1", "Services panel"),
        ("2", "Logs panel"),
        ("3", "Nodes panel"),
        ("Enter", "Service detail view"),
        ("Esc", "Back / clear filter"),
        ("/", "Filter services by name"),
        ("d", "Deploy (redeploy) service"),
        ("x", "Stop service"),
        ("r", "Refresh immediately"),
        ("l", "Show logs for service"),
        ("n", "Show nodes panel"),
        ("?", "Toggle this help"),
        ("q", "Quit"),
    ];

    let lines: Vec<Line> = bindings
        .into_iter()
        .map(|(key, desc)| {
            Line::from(vec![
                Span::styled(format!("  {key:<14}"), key_style),
                Span::styled(desc, desc_style),
            ])
        })
        .collect();

    let para = Paragraph::new(lines).block(block);
    f.render_widget(para, popup);
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
