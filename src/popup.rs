//! Daemon announcements as a short tmux popup under the status bar.
//!
//! A popup takes keyboard input while it is open, so one is only opened on a client whose
//! user has not typed for `IDLE_SECS`. Until then the announcement waits.
use crate::diagnostics::Issue;
use crate::tmux::{self, Client};
use anyhow::{Context, Result};
use std::collections::BTreeSet;
use std::process::{Child, Command, Stdio};

const IDLE_SECS: u64 = 3;
const VISIBLE_SECS: u32 = 5;

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
    let command = format!("printf '%s' \"$TAP_MESSAGE\"; sleep {VISIBLE_SECS}");
    Command::new("tmux")
        .args(["display-popup", "-c", client, "-E", "-e", &message_env])
        .args(["-T", " tmux-agent-plane "])
        .args(["-x", "C", "-w", "70%", "-h", "6"])
        .args(["-y", "#{?#{==:#{status-position},top},1,0}"])
        .arg(command)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to execute tmux")
}

#[cfg(test)]
mod tests {
    use super::*;

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
