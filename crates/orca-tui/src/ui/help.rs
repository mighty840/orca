//! Full-screen help view with grouped keybindings (k9s style).

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::state::AppState;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const COMMIT: &str = env!("ORCA_COMMIT");

/// Draw full-screen help view with grouped keybindings.
pub fn draw_help(f: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default()
        .title(format!(" Orca v{VERSION}-{COMMIT} -- Help "))
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
        ("0", "Chat with the cluster AI (the landing view)"),
        ("1", "Services view (grouped by project)"),
        ("2 / n", "Nodes view (CPU/Mem/Disk/Net sparklines)"),
        ("3", "Secrets view (grouped by inferred scope + ref counts)"),
        (
            "a / e / x (secrets)",
            "Add (:set prefill), edit selected key, delete with y/N confirm",
        ),
        (
            "p (secrets)",
            "Cycle scope filter: all -> global -> per-project -> broken refs",
        ),
        (
            "↵ on a secret row",
            "Drill into the list of services referencing that key",
        ),
        ("4", "Backups view (per-node snapshot status)"),
        (
            "↵ on a backup row",
            "Drill into snapshot list for that node",
        ),
        ("5", "Webhooks view (registered push triggers + history)"),
        (
            "6",
            "Networks view (per-node Docker bridges + public-edge routes)",
        ),
        (
            "↵ on a webhook row",
            "Drill into invocation history for that webhook",
        ),
        ("7", "Alerts view (AI alert conversations)"),
        (
            "8",
            "Cluster-token rotation: s start, f finish, F force finish (each asks y/N)",
        ),
        (
            "a / d / R (alerts)",
            "Show all incl. resolved / dismiss / resolve the selected alert",
        ),
        (
            "l",
            "Logs for selected service: live for services on the master, polled every 2 s for an agent's",
        ),
        ("?", "This help screen (j/k to scroll)"),
    ] {
        lines.push(bind(k, d, ks, ds));
    }

    // Actions
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("  Actions", hs)));
    for (k, d) in [
        ("s", "Scale service (opens :scale prompt)"),
        ("x", "Stop selected service (asks y/N)"),
        (
            "u",
            "Start (resume) a stopped service; undrain the node in Nodes",
        ),
        ("d", "Redeploy the service (asks y/N)"),
        ("x (nodes)", "Drain the selected node (asks y/N)"),
        ("c", "Collapse / expand the selected project"),
        ("p", "Filter by project of selected"),
        ("r", "Refresh immediately"),
        ("b", "Trigger backup on selected node (Backups view)"),
        (
            "a / e / x",
            "Add / edit / delete webhook (Webhooks view; x asks y/N)",
        ),
        (
            "/",
            "Filter the list (Services, Secrets, Webhooks, Alerts) or search the log (Logs)",
        ),
        ("w", "Toggle word wrap (in logs)"),
    ] {
        lines.push(bind(k, d, ks, ds));
    }

    // Commands
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled("  Commands (:)", hs)));
    for (k, d) in [
        (":scale <svc> <n>", "Scale service to n replicas"),
        (":stop <svc>", "Stop a service (asks y/N)"),
        (":start <svc>", "Start (resume) a stopped service"),
        (
            ":redeploy <svc>",
            "Pull the image and recreate the containers (asks y/N)",
        ),
        (
            ":rollback <svc>",
            "Roll back to the previous deploy (asks y/N)",
        ),
        (
            ":promote <svc>",
            "Promote canary instances to stable (asks y/N)",
        ),
        (":stop-project <p>", "Stop entire project (asks y/N)"),
        (":logs <svc>", "Open a service's logs"),
        (":chat / :nodes / :help", "Open those views"),
        (":filter <text>", "Filter services"),
        (":project <name>", "Filter by project"),
        (":secrets", "Open the secrets view"),
        (":backups", "Open the cluster backups dashboard"),
        (":webhooks", "Open the webhooks dashboard"),
        (":networks", "Open the cluster networks view"),
        (":alerts", "Open the alerts view"),
        (":token", "Open the cluster-token rotation view"),
        (
            ":token-rotate / :token-finish [--force]",
            "Start or finish a rotation (asks y/N)",
        ),
        (":reply <msg>", "Answer the AI in the open alert"),
        (":dismiss / :resolve", "Dismiss or resolve the open alert"),
        (
            ":webhook-add <repo> <branch> <svc> [--secret X] [--infra]",
            "Register a webhook",
        ),
        (
            ":webhook-edit <repo> <branch> <svc> [flags]",
            "Update an existing webhook (replaces by repo+branch+service)",
        ),
        (":webhook-rm <service>", "Remove a webhook"),
        (":set <KEY> <val>", "Create or update a secret"),
        (":rm <KEY>", "Remove a secret (asks y/N)"),
        (":drain <id>", "Drain a node (asks y/N)"),
        (":undrain <id>", "Undrain a node"),
        (
            ":sh [svc]",
            "Interactive shell in the selected (or named) service",
        ),
        (
            ":exec <svc> <cmd>",
            "Run a command in a service's container",
        ),
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

    let visible = (area.height as usize).saturating_sub(2);
    let scroll = state.help_scroll.min(lines.len().saturating_sub(visible));
    let para = Paragraph::new(lines)
        .block(block)
        .scroll((scroll as u16, 0));
    f.render_widget(para, area);
}

fn bind<'a>(key: &'a str, desc: &'a str, key_style: Style, desc_style: Style) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!("    {key:<20}"), key_style),
        Span::styled(desc, desc_style),
    ])
}
