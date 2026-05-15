//! Chat landing view — `orca ask`-style conversation with the cluster AI.
//!
//! Transcript pane on top, single-line composition box at bottom. The user
//! types into the box (Enter sends, Esc clears). The body of POST
//! `/api/v1/ask` is `{question, history}`; the server gathers cluster context.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::state::{AppState, ChatRole};

const HINT: &str = "Ask anything: \"why is api down?\", \"how do I scale the worker?\". Slash: /services /nodes /alerts /logs <svc>";

pub fn draw_chat(f: &mut Frame, area: Rect, state: &AppState) {
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(3)])
        .split(area);
    draw_transcript(f, layout[0], state);
    draw_input(f, layout[1], state);
}

fn draw_transcript(f: &mut Frame, area: Rect, state: &AppState) {
    if state.chat_unavailable {
        let block = Block::default()
            .title(" Chat (unavailable) ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow));
        let text = "  AI is not configured.\n\n  Add an [ai] block to cluster.toml (endpoint, model, api_key) and restart the server.";
        f.render_widget(Paragraph::new(text).block(block), area);
        return;
    }

    let lines: Vec<Line> = if state.chat.is_empty() {
        vec![
            Line::from(""),
            Line::from(Span::styled(
                "  Welcome — start chatting with your cluster.",
                Style::default().fg(Color::Cyan),
            )),
            Line::from(""),
            Line::from(Span::styled(
                format!("  {HINT}"),
                Style::default().fg(Color::DarkGray),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "  Tab/digit keys (1-6) jump to other views; the transcript stays put for the session.",
                Style::default().fg(Color::DarkGray),
            )),
        ]
    } else {
        let mut out: Vec<Line> = Vec::with_capacity(state.chat.len() * 4);
        for turn in &state.chat {
            let (label, color) = match turn.role {
                ChatRole::User => ("you", Color::Green),
                ChatRole::Assistant => ("orca", Color::Cyan),
            };
            out.push(Line::from(Span::styled(
                format!("{label}:"),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            )));
            for content_line in turn.content.lines() {
                out.push(Line::from(format!("  {content_line}")));
            }
            out.push(Line::from(""));
        }
        if state.chat_pending {
            out.push(Line::from(Span::styled(
                "orca: thinking…",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::ITALIC),
            )));
        }
        out
    };

    // `chat_scroll` is "lines back from the latest" — 0 pins the view to
    // the bottom (the latest exchange). To render with `Paragraph::scroll`
    // (which takes an offset from the top), compute the offset such that
    // the last visible row is `chat_scroll` lines above the actual last
    // line. The transcript pane height excludes the top/bottom borders.
    let visible = (area.height as usize).saturating_sub(2).max(1);
    let total = lines.len();
    let from_top = total
        .saturating_sub(visible)
        .saturating_sub(state.chat_scroll);
    let suffix = if state.chat_scroll > 0 {
        format!(
            " — scrolled up {} lines (PgDn / Down to follow)",
            state.chat_scroll
        )
    } else {
        String::new()
    };
    let block = Block::default()
        .title(format!(" Chat ({} turns){suffix} ", state.chat.len()))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let para = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((from_top as u16, 0));
    f.render_widget(para, area);
}

fn draw_input(f: &mut Frame, area: Rect, state: &AppState) {
    let title = if state.chat_pending {
        " ⠿ waiting for AI… "
    } else {
        " > type and press Enter "
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(if state.chat_pending {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default().fg(Color::Green)
        });
    let body = if state.chat_pending && state.chat_input.is_empty() {
        " (input disabled while AI is responding — Ctrl+C still quits)".to_string()
    } else {
        format!(" {}", state.chat_input)
    };
    f.render_widget(Paragraph::new(body).block(block), area);
}
