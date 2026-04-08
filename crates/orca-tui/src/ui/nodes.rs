//! Full-screen node table (k9s style).

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols;
use ratatui::widgets::{Block, Borders, Paragraph, Row, Sparkline, Table};

use crate::state::AppState;

/// Draw the full-screen nodes view: a table at the top, then a sparkline
/// strip per node showing CPU/mem/disk/IO history. Uses the same rolling
/// buffer the services view uses.
pub fn draw_nodes(f: &mut Frame, area: Rect, state: &AppState) {
    if state.nodes.is_empty() {
        let block = Block::default()
            .title(" Nodes (0) ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));
        let para = Paragraph::new("  No nodes registered (single-node mode)").block(block);
        f.render_widget(para, area);
        return;
    }

    let n_nodes = state.nodes.len() as u16;
    let spark_height: u16 = 5;
    // Reserve enough rows for the table itself, then split the rest between
    // each node's sparkline strip.
    let table_height = (area.height.saturating_sub(spark_height * n_nodes)).max(6);
    let mut constraints: Vec<Constraint> = vec![Constraint::Length(table_height)];
    for _ in 0..n_nodes {
        constraints.push(Constraint::Length(spark_height));
    }
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    draw_table(f, chunks[0], state);
    for (i, node) in state.nodes.iter().enumerate() {
        if let Some(rect) = chunks.get(i + 1) {
            draw_node_sparklines(f, *rect, state, node);
        }
    }
}

fn draw_table(f: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default()
        .title(format!(" Nodes ({}) ", state.nodes.len()))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let rows: Vec<Row> = state
        .nodes
        .iter()
        .map(|n| {
            let (relative, stale) = format_relative_heartbeat(&n.last_heartbeat);
            let drain_str = if n.drain { "draining" } else { "" };
            let labels_str = format_labels(&n.labels);

            let status_color = if n.drain {
                Color::Yellow
            } else if stale {
                Color::DarkGray
            } else {
                Color::Green
            };
            let status_text = if n.drain {
                "draining"
            } else if stale {
                "stale"
            } else {
                "ready"
            };

            Row::new(vec![
                n.node_id.to_string(),
                n.address.clone(),
                status_text.to_string(),
                drain_str.to_string(),
                relative,
                labels_str,
            ])
            .style(Style::default().fg(status_color))
        })
        .collect();

    let header = Row::new(vec![
        "ID",
        "ADDRESS",
        "STATUS",
        "DRAIN",
        "HEARTBEAT",
        "LABELS",
    ])
    .style(
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );
    let widths = [
        Constraint::Length(8),
        Constraint::Min(20),
        Constraint::Length(10),
        Constraint::Length(10),
        Constraint::Length(12),
        Constraint::Min(20),
    ];
    let table = Table::new(rows, widths).header(header).block(block);
    f.render_widget(table, area);
}

/// Render four side-by-side sparklines for a single node. Memory and disk
/// are scaled to the node's reported total so the sparkline shows a real
/// percentage, not an auto-scaled block. Network throughput is computed by
/// diffing consecutive samples of the cumulative byte counters.
fn draw_node_sparklines(f: &mut Frame, area: Rect, state: &AppState, node: &crate::api::NodeInfo) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
        ])
        .split(area);

    let history = state.node_history.get(&node.node_id);
    let cpu: Vec<u64> = history
        .map(|h| h.cpu.iter().map(|c| c.round() as u64).collect())
        .unwrap_or_default();
    let mem_mib: Vec<u64> = history
        .map(|h| h.mem_bytes.iter().map(|b| b / (1024 * 1024)).collect())
        .unwrap_or_default();
    let disk_mib: Vec<u64> = history
        .map(|h| h.disk_used.iter().map(|b| b / (1024 * 1024)).collect())
        .unwrap_or_default();
    // Convert cumulative rx+tx byte counters into a per-sample delta (KiB)
    // so the sparkline reflects "activity right now" rather than a
    // monotonically growing total.
    let net: Vec<u64> = history
        .map(|h| {
            h.net_rx
                .iter()
                .zip(h.net_tx.iter())
                .collect::<Vec<_>>()
                .windows(2)
                .map(|w| {
                    let (prev_rx, prev_tx) = w[0];
                    let (cur_rx, cur_tx) = w[1];
                    let delta_rx = cur_rx.saturating_sub(*prev_rx);
                    let delta_tx = cur_tx.saturating_sub(*prev_tx);
                    (delta_rx + delta_tx) / 1024
                })
                .collect()
        })
        .unwrap_or_default();

    let mem_total_mib = node.memory_total / (1024 * 1024);
    let disk_total_mib = node.disk_total / (1024 * 1024);
    let cur_mem = mem_mib.last().copied().unwrap_or(0);
    let cur_disk = disk_mib.last().copied().unwrap_or(0);

    // Master node is labeled differently so an operator glancing at the
    // screen can tell which strip is which.
    let role = node
        .labels
        .get("role")
        .map(|r| r.as_str())
        .unwrap_or("node");
    let prefix = format!(" [{role}] {} ", node.address);

    let cpu_title = format!("{prefix}CPU% ({:.0}%) ", node.cpu_percent);
    let mem_title = format!(" Mem {}/{} MiB ", cur_mem, mem_total_mib);
    let disk_title = format!(" Disk {}/{} MiB ", cur_disk, disk_total_mib);
    let net_title = " Net KiB/s (delta) ";

    spark(f, cols[0], &cpu, &cpu_title, Color::Cyan, Some(100));
    spark(
        f,
        cols[1],
        &mem_mib,
        &mem_title,
        Color::Magenta,
        Some(mem_total_mib.max(1)),
    );
    spark(
        f,
        cols[2],
        &disk_mib,
        &disk_title,
        Color::Yellow,
        Some(disk_total_mib.max(1)),
    );
    spark(f, cols[3], &net, net_title, Color::Green, None);
}

fn spark(f: &mut Frame, area: Rect, data: &[u64], title: &str, color: Color, max: Option<u64>) {
    let block = Block::default()
        .title(title.to_string())
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    let mut widget = Sparkline::default()
        .block(block)
        .data(data)
        .style(Style::default().fg(color))
        .bar_set(symbols::bar::NINE_LEVELS);
    if let Some(m) = max {
        widget = widget.max(m);
    }
    f.render_widget(widget, area);
}

/// Format node labels as key=value pairs.
fn format_labels(labels: &std::collections::HashMap<String, String>) -> String {
    if labels.is_empty() {
        return "-".to_string();
    }
    let mut pairs: Vec<String> = labels.iter().map(|(k, v)| format!("{k}={v}")).collect();
    pairs.sort();
    pairs.join(", ")
}

/// Parse an ISO 8601 heartbeat timestamp and return relative time + staleness.
fn format_relative_heartbeat(ts: &str) -> (String, bool) {
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    if let Some(ts_secs) = parse_iso_timestamp(ts) {
        let diff = now_secs.saturating_sub(ts_secs);
        let stale = diff > 30;
        let relative = if diff < 60 {
            format!("{diff}s ago")
        } else if diff < 3600 {
            format!("{}m ago", diff / 60)
        } else {
            format!("{}h ago", diff / 3600)
        };
        (relative, stale)
    } else {
        (ts.chars().take(19).collect(), false)
    }
}

/// Minimal ISO 8601 parser -> unix seconds.
fn parse_iso_timestamp(ts: &str) -> Option<u64> {
    let ts = ts.trim_end_matches('Z').trim();
    if ts.len() < 19 {
        return None;
    }
    let year: u64 = ts[0..4].parse().ok()?;
    let month: u64 = ts[5..7].parse().ok()?;
    let day: u64 = ts[8..10].parse().ok()?;
    let hour: u64 = ts[11..13].parse().ok()?;
    let min: u64 = ts[14..16].parse().ok()?;
    let sec: u64 = ts[17..19].parse().ok()?;

    let mut days: u64 = 0;
    for y in 1970..year {
        days += if is_leap(y) { 366 } else { 365 };
    }
    let month_days = [0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    for m in 1..month {
        days += month_days[m as usize];
        if m == 2 && is_leap(year) {
            days += 1;
        }
    }
    days += day - 1;

    Some(days * 86400 + hour * 3600 + min * 60 + sec)
}

fn is_leap(y: u64) -> bool {
    (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400)
}
