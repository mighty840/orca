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

    let (table_height, strips) =
        sparkline_window(area.height, state.nodes.len(), state.selected_node);
    let mut constraints: Vec<Constraint> = vec![Constraint::Length(table_height)];
    for _ in strips.clone() {
        constraints.push(Constraint::Length(SPARK_HEIGHT));
    }
    constraints.push(Constraint::Min(0));
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    draw_table(f, chunks[0], state);
    for (slot, i) in strips.enumerate() {
        if let (Some(rect), Some(node)) = (chunks.get(slot + 1), state.nodes.get(i)) {
            draw_node_sparklines(f, *rect, state, node);
        }
    }
}

/// Rows per node's sparkline strip.
const SPARK_HEIGHT: u16 = 5;

/// Table height and which nodes get a sparkline strip in `height` rows.
/// The table gets a row per node (plus border and header) up to half the
/// screen; strips fill what's left, centred on the selected node. Before,
/// every node got a strip, and past ~6 nodes the layout squeezed them all
/// into nothing (#265).
fn sparkline_window(height: u16, nodes: usize, selected: usize) -> (u16, std::ops::Range<usize>) {
    let table = (nodes as u16 + 3).min(height / 2).max(6.min(height));
    let fit = (height.saturating_sub(table) / SPARK_HEIGHT) as usize;
    let fit = fit.min(nodes);
    let first = selected
        .saturating_sub(fit / 2)
        .min(nodes.saturating_sub(fit));
    (table, first..first + fit)
}

fn draw_table(f: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default()
        .title(format!(" Nodes ({}) ", state.nodes.len()))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let rows: Vec<Row> = state
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| {
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
            .style(if i == state.selected_node {
                Style::default()
                    .fg(status_color)
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(status_color)
            })
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

/// RFC 3339 heartbeat timestamp -> unix seconds. The hand-rolled parser
/// this replaces indexed a month table by the parsed month and panicked on
/// month 13 or day 0 (#264).
fn parse_iso_timestamp(ts: &str) -> Option<u64> {
    let t = chrono::DateTime::parse_from_rfc3339(ts.trim()).ok()?;
    u64::try_from(t.timestamp()).ok()
}

#[cfg(test)]
mod tests {
    use super::{parse_iso_timestamp, sparkline_window};

    #[test]
    fn strips_fit_the_screen_and_follow_the_selection() {
        // 40 rows, 12 nodes: a 15-row table, 5 strips around node 9.
        let (table, strips) = sparkline_window(40, 12, 9);
        assert_eq!(table, 15);
        assert_eq!(strips, 7..12);
        // Two nodes on a tall screen: both get a strip.
        assert_eq!(sparkline_window(50, 2, 0).1, 0..2);
        // A short screen: the table only.
        assert_eq!(sparkline_window(10, 3, 0).1.len(), 0);
    }

    #[test]
    fn heartbeat_timestamps_parse() {
        assert_eq!(parse_iso_timestamp("1970-01-02T00:00:00Z"), Some(86_400));
        assert!(parse_iso_timestamp("2026-09-28T07:14:03.123456789Z").is_some());
    }

    #[test]
    fn malformed_timestamps_are_none_not_a_panic() {
        // The old parser indexed a month table with month 13 and did
        // `day - 1` on day 0.
        for ts in [
            "2026-13-01T00:00:00Z",
            "2026-01-00T00:00:00Z",
            "garbage",
            "",
            "2026-é1-01T00:00:00Z",
        ] {
            assert_eq!(parse_iso_timestamp(ts), None, "{ts}");
        }
    }
}
