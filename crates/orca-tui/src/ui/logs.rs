//! Log panel rendering.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::state::{AppState, Panel};

/// Draw the logs panel with line numbers and auto-scroll.
pub fn draw_logs(f: &mut Frame, area: Rect, state: &AppState) {
    let border_style = if state.panel == Panel::Logs {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let title = state
        .selected_service_name()
        .map(|n| format!(" Logs: {n} "))
        .unwrap_or_else(|| " Logs ".into());

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(border_style);

    // Available width inside the block borders.
    let inner_w = if area.width > 2 {
        (area.width - 2) as usize
    } else {
        0
    };

    // Available height inside the block borders.
    let inner_h = if area.height > 2 {
        (area.height - 2) as usize
    } else {
        0
    };

    let log_lines: Vec<&str> = state.logs.lines().collect();
    let total = log_lines.len();

    // Auto-scroll: show the last `inner_h` lines.
    let start = total.saturating_sub(inner_h);

    let line_num_width = format!("{}", total).len();

    let lines: Vec<Line> = log_lines[start..]
        .iter()
        .enumerate()
        .map(|(i, text)| {
            let num = start + i + 1;
            let num_str = format!("{:>width$} ", num, width = line_num_width);
            // Truncate to terminal width.
            let max_text = if inner_w > num_str.len() {
                inner_w - num_str.len()
            } else {
                0
            };
            let truncated: String = text.chars().take(max_text).collect();
            Line::from(vec![
                Span::styled(
                    num_str,
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::DIM),
                ),
                Span::raw(truncated),
            ])
        })
        .collect();

    let para = Paragraph::new(lines).block(block);
    f.render_widget(para, area);
}
