//! Daemon announcements in their own floating window, a little below the top of the terminal.
//!
//! A popup takes keyboard input while it is open, so one is only opened on a client whose
//! user has not typed for `IDLE_SECS`. Until then the announcement waits.
use crate::diagnostics::Issue;
use crate::index::State;
use crate::tmux::{self, Client};
use crate::ui::{LAVENDER, OVERLAY, TEXT, colour};
use anyhow::{Context, Result};
use ratatui::Frame;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::layout::{Constraint, Layout, Margin};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use std::collections::BTreeSet;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const IDLE_SECS: u64 = 3;
const VISIBLE: Duration = Duration::from_secs(10);
const POPUP_ROWS: &str = "10";
/// `display-popup -y` names the popup's bottom row: 13 puts a 10-row popup's top at row 4,
/// clear of a status line on the first row.
const POPUP_BOTTOM_ROW: &str = "13";

struct Pending {
    message: String,
    clients: BTreeSet<String>,
}

#[derive(Default)]
pub struct Announcements {
    pending: Vec<Pending>,
    open: Vec<(String, Child)>,
}

impl Announcements {
    /// Queue a message for every currently attached client.
    pub fn push(&mut self, message: String) -> Result<()> {
        let clients = tmux::clients()?.into_iter().map(|c| c.name).collect();
        self.pending.push(Pending { message, clients });
        Ok(())
    }

    /// Open due popups and report popups that failed. Call once per tick.
    pub fn deliver(&mut self) -> Vec<Issue> {
        let mut issues = Vec::new();
        self.open
            .retain_mut(|(client, child)| match child.try_wait() {
                Ok(None) => true,
                Ok(Some(status)) if status.success() => false,
                Ok(Some(status)) => {
                    issues.push(Issue::new(
                        "diagnostics:tmux",
                        anyhow::anyhow!("popup on {client} failed: {status}"),
                    ));
                    false
                }
                Err(error) => {
                    issues.push(Issue::new("diagnostics:tmux", error.into()));
                    false
                }
            });
        if self.pending.is_empty() {
            return issues;
        }
        let clients = match tmux::clients() {
            Ok(clients) => clients,
            Err(error) => {
                issues.push(Issue::new("diagnostics:tmux", error));
                return issues;
            }
        };
        let busy: BTreeSet<String> = self.open.iter().map(|(c, _)| c.clone()).collect();
        for (client, index) in due(&self.pending, &clients, &busy, tmux::now_secs()) {
            match show(&client, &self.pending[index].message) {
                Ok(child) => {
                    self.pending[index].clients.remove(&client);
                    self.open.push((client, child));
                }
                Err(error) => issues.push(Issue::new("diagnostics:tmux", error)),
            }
        }
        let live: BTreeSet<_> = clients.into_iter().map(|c| c.name).collect();
        for pending in &mut self.pending {
            pending.clients.retain(|c| live.contains(c));
        }
        self.pending.retain(|p| !p.clients.is_empty());
        issues
    }
}

/// One popup per idle client without an open popup, taking its oldest waiting message.
fn due(
    pending: &[Pending],
    clients: &[Client],
    busy: &BTreeSet<String>,
    now: u64,
) -> Vec<(String, usize)> {
    clients
        .iter()
        .filter(|c| !busy.contains(&c.name) && now.saturating_sub(c.activity) >= IDLE_SECS)
        .filter_map(|c| {
            let index = pending.iter().position(|p| p.clients.contains(&c.name))?;
            Some((c.name.clone(), index))
        })
        .collect()
}

/// Open the popup on one client without waiting: `display-popup` returns when it closes.
pub fn show(client: &str, message: &str) -> Result<Child> {
    let message_env = format!("TAP_MESSAGE={message}");
    Command::new("tmux")
        .args(["display-popup", "-c", client, "-E", "-e", &message_env])
        .args(["-T", " tmux-agent-plane ", "-b", "rounded"])
        .args(["-S", "fg=#585b70", "-s", "bg=#1e1e2e,fg=#cdd6f4"])
        .args(["-x", "C", "-w", "60%", "-h", POPUP_ROWS])
        .args(["-y", POPUP_BOTTOM_ROW])
        .arg("tmux-agent-plane notice")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to execute tmux")
}

/// One daemon message, split into what a reader looks for first.
#[derive(Debug, PartialEq, Eq)]
struct Notice {
    heading: &'static str,
    ok: bool,
    /// Each change as its source and what happened.
    changes: Vec<(String, String)>,
    log: Option<String>,
}

fn parse_notice(message: &str) -> Notice {
    let (body, log) = match message.rsplit_once("; log: ") {
        Some((body, log)) => (body, Some(log.to_string())),
        None => (message, None),
    };
    let stopped = body.strip_prefix("tmux-agent-plane stopped: ");
    let body = stopped.unwrap_or_else(|| body.strip_prefix("tmux-agent-plane: ").unwrap_or(body));
    let changes: Vec<(String, String)> = body
        .split("; ")
        .map(|change| match change.split_once(": ") {
            Some((source, detail)) => (source.to_string(), detail.to_string()),
            None => (String::new(), change.to_string()),
        })
        .collect();
    let ok = stopped.is_none() && changes.iter().all(|(_, detail)| detail == "recovered");
    Notice {
        heading: match (stopped.is_some(), ok) {
            (true, _) => "⚠ tmux-agent-plane stopped",
            (false, true) => "✓ Recovered",
            (false, false) => "⚠ Something needs attention",
        },
        ok,
        changes,
        log,
    }
}

fn draw_notice(frame: &mut Frame, notice: &Notice, left: u64) {
    let area = frame.area().inner(Margin::new(2, 0));
    let [body, hint] = Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(area);
    let tone = colour(if notice.ok {
        State::Done
    } else {
        State::Blocked
    });
    let mut lines = vec![
        Line::styled(notice.heading, Style::default().fg(tone).bold()),
        Line::default(),
    ];
    for (source, detail) in &notice.changes {
        lines.push(Line::from(vec![
            Span::styled(format!("{source} "), LAVENDER),
            Span::styled(detail.clone(), TEXT),
        ]));
    }
    if let Some(log) = &notice.log {
        lines.push(Line::default());
        lines.push(Line::styled(format!("Log: {log}"), OVERLAY));
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body);
    frame.render_widget(
        Paragraph::new(format!("any key dismisses · closes in {left}s")).style(OVERLAY),
        hint,
    );
}

/// The popup's own process: show the message until a key press or `VISIBLE`.
pub fn notice() -> Result<()> {
    let message = std::env::var("TAP_MESSAGE").context("TAP_MESSAGE is not set")?;
    let notice = parse_notice(&message);
    let mut terminal = ratatui::init();
    let result = (|| -> Result<()> {
        let end = Instant::now() + VISIBLE;
        while let Some(left) = end.checked_duration_since(Instant::now()) {
            let shown = left.as_secs() + 1;
            terminal.draw(|frame| draw_notice(frame, &notice, shown))?;
            // Wake on the second, so the countdown stays true.
            if event::poll(left.min(Duration::from_millis(left.subsec_millis() as u64 + 1)))?
                && matches!(event::read()?, Event::Key(key) if key.kind == KeyEventKind::Press)
            {
                break;
            }
        }
        Ok(())
    })();
    ratatui::restore();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notices_split_heading_changes_and_log() {
        let notice = parse_notice("tmux-agent-plane: scan:pi: recovered; log: /tmp/d.log");
        assert_eq!(notice.heading, "✓ Recovered");
        assert!(notice.ok);
        assert_eq!(
            notice.changes,
            [("scan:pi".to_string(), "recovered".to_string())]
        );
        assert_eq!(notice.log.as_deref(), Some("/tmp/d.log"));
        let notice = parse_notice(
            "tmux-agent-plane: scan:pi: recovered; notification:desktop: failed: 23; log: /l",
        );
        assert!(!notice.ok);
        assert_eq!(notice.changes.len(), 2);
        assert_eq!(notice.changes[1].1, "failed: 23");
        let notice = parse_notice("tmux-agent-plane stopped: signal 15; log: /l");
        assert_eq!(notice.heading, "⚠ tmux-agent-plane stopped");
        assert!(!notice.ok);
        assert_eq!(parse_notice("plain").changes[0].1, "plain");
    }

    #[test]
    fn notice_reads_at_a_narrow_width() {
        use ratatui::{Terminal, backend::TestBackend};
        let notice = parse_notice("tmux-agent-plane: scan:pi: recovered; log: /tmp/d.log");
        let mut terminal = Terminal::new(TestBackend::new(48, 8)).unwrap();
        terminal
            .draw(|frame| draw_notice(frame, &notice, 10))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let text: String = (0..8)
            .map(|y| (0..48).map(|x| buffer[(x, y)].symbol()).collect::<String>() + "\n")
            .collect();
        assert!(text.contains("✓ Recovered"));
        assert!(text.contains("scan:pi recovered"));
        assert!(text.contains("Log: /tmp/d.log"));
        assert!(text.contains("any key dismisses · closes in 10s"));
    }

    fn client(name: &str, activity: u64) -> Client {
        Client {
            name: name.into(),
            activity,
        }
    }

    fn pending(message: &str, clients: &[&str]) -> Pending {
        Pending {
            message: message.into(),
            clients: clients.iter().map(|c| c.to_string()).collect(),
        }
    }

    #[test]
    fn popups_wait_for_a_pause_in_typing_and_never_stack_on_a_client() {
        let pending = [pending("a", &["typing", "idle", "busy"])];
        let clients = [client("typing", 99), client("idle", 90), client("busy", 90)];
        let busy = BTreeSet::from(["busy".to_string()]);
        assert_eq!(due(&pending, &clients, &busy, 100), [("idle".into(), 0)]);
    }

    #[test]
    fn a_client_receives_its_oldest_waiting_message_first() {
        let pending = [
            pending("a", &["other"]),
            pending("b", &["c"]),
            pending("c", &["c"]),
        ];
        let clients = [client("c", 0)];
        assert_eq!(
            due(&pending, &clients, &BTreeSet::new(), 100),
            [("c".into(), 1)]
        );
    }
}
