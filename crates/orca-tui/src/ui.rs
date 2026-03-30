//! TUI rendering with ratatui.

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Row, Table, Wrap};

use crate::state::{AppState, Panel};

/// Render the full dashboard.
pub fn draw(f: &mut Frame, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // header
            Constraint::Min(10),   // main
            Constraint::Length(1), // footer
        ])
        .split(f.area());

    draw_header(f, chunks[0], state);
    draw_main(f, chunks[1], state);
    draw_footer(f, chunks[2], state);
}

fn draw_header(f: &mut Frame, area: Rect, state: &AppState) {
    let running = state
        .services
        .iter()
        .filter(|s| s.status == "running")
        .count();
    let total = state.services.len();
    let text = Line::from(vec![
        Span::styled(
            " orca ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(
            "| {} | Services: {}/{} | Nodes: {}",
            state.cluster_name, running, total, state.node_count
        )),
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

    draw_services(f, chunks[0], state);

    match state.panel {
        Panel::Services | Panel::Logs => draw_logs(f, chunks[1], state),
        Panel::Nodes => draw_nodes(f, chunks[1], state),
    }
}

fn draw_services(f: &mut Frame, area: Rect, state: &AppState) {
    let highlight = Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD);
    let border_style = if state.panel == Panel::Services {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let items: Vec<ListItem> = state
        .services
        .iter()
        .enumerate()
        .map(|(i, svc)| {
            let status_color = match svc.status.as_str() {
                "running" => Color::Green,
                "degraded" => Color::Yellow,
                _ => Color::Red,
            };
            let marker = if i == state.selected_service {
                "> "
            } else {
                "  "
            };
            let line = Line::from(vec![
                Span::raw(marker),
                Span::styled(
                    &svc.name,
                    if i == state.selected_service {
                        highlight
                    } else {
                        Style::default()
                    },
                ),
                Span::raw(format!(
                    " {}/{} ",
                    svc.running_replicas, svc.desired_replicas
                )),
                Span::styled(&svc.status, Style::default().fg(status_color)),
            ]);
            ListItem::new(line)
        })
        .collect();

    let block = Block::default()
        .title(" Services ")
        .borders(Borders::ALL)
        .border_style(border_style);
    let list = List::new(items).block(block);
    f.render_widget(list, area);
}

fn draw_logs(f: &mut Frame, area: Rect, state: &AppState) {
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
    let para = Paragraph::new(state.logs.as_str())
        .block(block)
        .wrap(Wrap { trim: false });
    f.render_widget(para, area);
}

fn draw_nodes(f: &mut Frame, area: Rect, state: &AppState) {
    let border_style = Style::default().fg(Color::Cyan);
    let block = Block::default()
        .title(" Nodes ")
        .borders(Borders::ALL)
        .border_style(border_style);

    if state.nodes.is_empty() {
        let para = Paragraph::new("No nodes registered (single-node mode)").block(block);
        f.render_widget(para, area);
        return;
    }

    let rows: Vec<Row> = state
        .nodes
        .iter()
        .map(|n| {
            Row::new(vec![
                n.node_id.to_string(),
                n.address.clone(),
                n.last_heartbeat.chars().take(19).collect::<String>(),
            ])
        })
        .collect();

    let header = Row::new(vec!["ID", "ADDRESS", "LAST HEARTBEAT"])
        .style(Style::default().add_modifier(Modifier::BOLD));
    let widths = [
        Constraint::Length(20),
        Constraint::Length(25),
        Constraint::Min(20),
    ];
    let table = Table::new(rows, widths).header(header).block(block);
    f.render_widget(table, area);
}

fn draw_footer(f: &mut Frame, area: Rect, state: &AppState) {
    let text = if let Some(err) = &state.error {
        Line::from(Span::styled(err, Style::default().fg(Color::Red)))
    } else {
        Line::from(vec![
            Span::styled(" j/k", Style::default().fg(Color::Cyan)),
            Span::raw(" nav  "),
            Span::styled("Tab", Style::default().fg(Color::Cyan)),
            Span::raw(" panel  "),
            Span::styled("l", Style::default().fg(Color::Cyan)),
            Span::raw(" logs  "),
            Span::styled("n", Style::default().fg(Color::Cyan)),
            Span::raw(" nodes  "),
            Span::styled("q", Style::default().fg(Color::Cyan)),
            Span::raw(" quit"),
        ])
    };
    f.render_widget(Paragraph::new(text), area);
}
