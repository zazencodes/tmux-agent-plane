//! Who spawned whom. `spawn` and `restore` put the parent agent's pane id on the child's pane in
//! `@agent-plane-parent`. The first tick that sees both agents binds the link to their processes,
//! `<parent pane> <parent pid> <child pid>`, so it lasts exactly as long as the child agent and
//! says when the parent has ended. A link never moves to whatever runs in a pane later.
use crate::diagnostics::Issue;
use crate::index::Agent;
use crate::tmux::Pane;
use anyhow::{Result, anyhow, bail};
use std::collections::HashMap;

pub const OPTION: &str = "@agent-plane-parent";

#[derive(Debug, PartialEq)]
enum Link<'a> {
    Unbound(&'a str),
    Bound {
        parent: &'a str,
        parent_pid: i32,
        child_pid: i32,
    },
}

fn parse(value: &str) -> Result<Option<Link<'_>>> {
    let fields: Vec<&str> = value.split(' ').collect();
    let pane = |s: &str| s.starts_with('%') && s[1..].parse::<u32>().is_ok();
    Ok(match fields.as_slice() {
        [""] => None,
        [parent] if pane(parent) => Some(Link::Unbound(parent)),
        [parent, parent_pid, child_pid] if pane(parent) => Some(Link::Bound {
            parent,
            parent_pid: parent_pid.parse()?,
            child_pid: child_pid.parse()?,
        }),
        _ => bail!("invalid parent link {value:?}"),
    })
}

/// Set each detected agent's `parent` and `parent_ended` from its pane's link. `pids` holds the
/// agent process of every agent detected this tick; an agent withdrawn by a scan fault keeps what
/// it had. `write` sets a pane's link option, or unsets it with `None`.
pub fn resolve(
    agents: &mut [Agent],
    pids: &HashMap<String, i32>,
    panes: &[Pane],
    mut write: impl FnMut(&str, Option<&str>) -> Result<()>,
) -> Vec<Issue> {
    let mut issues = Vec::new();
    let tracked: Vec<String> = agents.iter().map(|a| a.pane.clone()).collect();
    for agent in agents.iter_mut() {
        let Some(&pid) = pids.get(&agent.pane) else {
            continue;
        };
        let raw = panes
            .iter()
            .find(|p| p.id == agent.pane)
            .map_or("", |p| p.parent.as_str());
        let (parent, ended) = match parse(raw) {
            Err(error) => {
                issues.push(Issue::new(format!("link:{}", agent.pane), error));
                (None, false)
            }
            Ok(None) => (None, false),
            // The parent is not running yet (restore brings agents back one by one).
            Ok(Some(Link::Unbound(parent))) => match pids.get(parent) {
                None => (None, false),
                Some(parent_pid) => {
                    let bound = format!("{parent} {parent_pid} {pid}");
                    match write(&agent.pane, Some(&bound)) {
                        Ok(()) => (Some(parent.to_string()), false),
                        Err(error) => {
                            issues.push(Issue::new(format!("link:{}", agent.pane), error));
                            (None, false)
                        }
                    }
                }
            },
            // Another agent runs in the pane now: the link belonged to the one before.
            Ok(Some(Link::Bound { child_pid, .. })) if child_pid != pid => {
                if let Err(error) = write(&agent.pane, None) {
                    issues.push(Issue::new(format!("link:{}", agent.pane), error));
                }
                (None, false)
            }
            Ok(Some(Link::Bound {
                parent, parent_pid, ..
            })) => match pids.get(parent) {
                Some(&live) if live == parent_pid => (Some(parent.to_string()), false),
                // Withdrawn by a scan fault: still the agent it was.
                None if tracked.iter().any(|p| p == parent) => (Some(parent.to_string()), false),
                _ => (None, true),
            },
        };
        agent.parent = parent;
        agent.parent_ended = ended;
    }
    issues.extend(break_cycles(agents));
    issues
}

/// Links point at agents that existed first, so a cycle means corrupt pane options.
fn break_cycles(agents: &mut [Agent]) -> Vec<Issue> {
    let parents: HashMap<String, String> = agents
        .iter()
        .filter_map(|a| Some((a.pane.clone(), a.parent.clone()?)))
        .collect();
    let mut issues = Vec::new();
    for agent in agents.iter_mut() {
        let mut current = agent.parent.as_deref();
        for _ in 0..parents.len() {
            match current {
                Some(pane) if pane == agent.pane => {
                    issues.push(Issue::new(
                        format!("link:{}", agent.pane),
                        anyhow!("parent links form a cycle through {}", agent.pane),
                    ));
                    agent.parent = None;
                    break;
                }
                Some(pane) => current = parents.get(pane).map(String::as_str),
                None => break,
            }
        }
    }
    issues
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::index::{Kind, State};

    pub(crate) fn agent(pane: &str) -> Agent {
        Agent {
            name: pane.trim_start_matches('%').into(),
            pane: pane.into(),
            kind: Kind::Claude,
            state: State::Idle,
            detail: None,
            title: String::new(),
            location: String::new(),
            session: "s".into(),
            workspace: "main".into(),
            window: "@1".into(),
            cwd: "/tmp".into(),
            parent: None,
            parent_ended: false,
            session_id: None,
            state_since: 0,
            completions: 0,
            reported_completions: None,
            completion_epoch: None,
            visible: false,
        }
    }

    fn pane(id: &str, parent: &str) -> Pane {
        Pane {
            id: id.into(),
            pid: 1,
            session: "s".into(),
            session_id: "$1".into(),
            workspace: "main".into(),
            window: "@1".into(),
            window_index: 1,
            pane_index: 0,
            cwd: "/tmp".into(),
            title: String::new(),
            active: false,
            viewed: false,
            sidebar: false,
            name: String::new(),
            parent: parent.into(),
            placed: false,
            task: false,
        }
    }

    fn run(
        agents: &mut [Agent],
        pids: &[(&str, i32)],
        panes: &[Pane],
    ) -> (Vec<Issue>, Vec<(String, Option<String>)>) {
        let pids = pids.iter().map(|(p, pid)| (p.to_string(), *pid)).collect();
        let mut writes = Vec::new();
        let issues = resolve(agents, &pids, panes, |pane, value| {
            writes.push((pane.to_string(), value.map(str::to_string)));
            Ok(())
        });
        (issues, writes)
    }

    #[test]
    fn parses_unbound_bound_and_rejects_anything_else() {
        assert_eq!(parse("").unwrap(), None);
        assert_eq!(parse("%3").unwrap(), Some(Link::Unbound("%3")));
        assert_eq!(
            parse("%3 10 20").unwrap(),
            Some(Link::Bound {
                parent: "%3",
                parent_pid: 10,
                child_pid: 20
            })
        );
        for bad in ["3", "%3 10", "%x", "%3 a 20", "%3 10 20 30"] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn an_unbound_link_binds_to_both_processes_once_both_run() {
        let mut agents = [agent("%1"), agent("%2")];
        let panes = [pane("%1", ""), pane("%2", "%1")];
        let (issues, writes) = run(&mut agents, &[("%1", 10), ("%2", 20)], &panes);
        assert!(issues.is_empty());
        assert_eq!(writes, [("%2".into(), Some("%1 10 20".into()))]);
        assert_eq!(agents[1].parent.as_deref(), Some("%1"));

        // Restore: the parent is not running yet, so the child waits without a parent.
        let mut agents = [agent("%2")];
        let (_, writes) = run(&mut agents, &[("%2", 20)], &panes);
        assert!(writes.is_empty());
        assert_eq!(agents[0].parent, None);
    }

    #[test]
    fn a_link_ends_with_the_child_and_reports_an_ended_parent() {
        let panes = [pane("%1", ""), pane("%2", "%1 10 20")];
        let mut agents = [agent("%1"), agent("%2")];
        let (_, writes) = run(&mut agents, &[("%1", 10), ("%2", 20)], &panes);
        assert!(writes.is_empty());
        assert_eq!(
            (agents[1].parent.as_deref(), agents[1].parent_ended),
            (Some("%1"), false)
        );

        // Someone started another agent in the parent's pane.
        let (_, _) = run(&mut agents, &[("%1", 11), ("%2", 20)], &panes);
        assert_eq!(
            (agents[1].parent.as_deref(), agents[1].parent_ended),
            (None, true)
        );

        // The child exited and another agent started in its pane by hand.
        let (_, writes) = run(&mut agents, &[("%1", 10), ("%2", 21)], &panes);
        assert_eq!(writes, [("%2".into(), None)]);
        assert_eq!(
            (agents[1].parent.as_deref(), agents[1].parent_ended),
            (None, false)
        );
    }

    #[test]
    fn a_withdrawn_parent_keeps_its_children_and_a_withdrawn_child_keeps_its_link() {
        let panes = [pane("%1", ""), pane("%2", "%1 10 20")];
        let mut agents = [agent("%1"), agent("%2")];
        run(&mut agents, &[("%2", 20)], &panes);
        assert_eq!(agents[1].parent.as_deref(), Some("%1"));
        agents[1].parent_ended = true;
        run(&mut agents, &[("%1", 10)], &panes);
        assert!(agents[1].parent_ended);
    }

    #[test]
    fn corrupt_links_and_cycles_fail_loudly() {
        let panes = [
            pane("%1", "%2 20 10"),
            pane("%2", "%1 10 20"),
            pane("%3", "junk"),
        ];
        let mut agents = [agent("%1"), agent("%2"), agent("%3")];
        let (issues, _) = run(&mut agents, &[("%1", 10), ("%2", 20), ("%3", 30)], &panes);
        assert_eq!(
            issues.len(),
            3,
            "{:?}",
            issues.iter().map(|i| &i.message).collect::<Vec<_>>()
        );
        assert!(agents.iter().all(|a| a.parent.is_none()));
    }
}
