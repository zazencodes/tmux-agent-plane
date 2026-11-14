//! One project role, stored on the tmux session and bound to an agent process. Every write is
//! compare-and-set inside tmux's command queue: late naming answers cannot undo explicit reports.
use crate::diagnostics::Issue;
use crate::index::{self, Agent, Kind};
use crate::{commands, detect, lifecycle, tmux};
use anyhow::{Context, Result, bail};
use clap::{Subcommand, ValueEnum};
use std::collections::HashSet;

pub const OPTION: &str = "@agent-plane-orchestrator";

#[derive(Subcommand)]
pub enum Action {
    /// Take charge of this agent's project, transferring the role from its previous holder
    Set { target: String },
    /// Report that this agent is stepping down; leaves a successor's role intact
    Clear { target: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Owner {
    pane: String,
    pid: i32,
    kind: Kind,
}

impl Owner {
    fn of(agent: &Agent) -> Self {
        Self {
            pane: agent.pane.clone(),
            pid: agent.process_id,
            kind: agent.kind,
        }
    }
    fn matches(&self, agent: &Agent) -> bool {
        self.pane == agent.pane && self.pid == agent.process_id && self.kind == agent.kind
    }
}

#[derive(Default, Debug, PartialEq, Eq)]
struct Record {
    revision: u64,
    owner: Option<Owner>,
}

impl Record {
    fn parse(raw: &str) -> Result<Self> {
        if raw.is_empty() {
            return Ok(Self::default());
        }
        let fields: Vec<_> = raw.split(' ').collect();
        let (revision, owner) = match fields.as_slice() {
            [revision] => (revision, None),
            [revision, pane, pid, kind]
                if pane.starts_with('%') && pane[1..].parse::<u32>().is_ok() =>
            {
                let pid: i32 = pid.parse().context("orchestrator process id")?;
                if pid <= 0 {
                    bail!("invalid orchestrator process id {pid}");
                }
                let kind = Kind::from_str(kind, false).map_err(anyhow::Error::msg)?;
                (
                    revision,
                    Some(Owner {
                        pane: pane.to_string(),
                        pid,
                        kind,
                    }),
                )
            }
            _ => bail!("invalid orchestrator assignment {raw:?}"),
        };
        Ok(Self {
            revision: revision.parse().context("orchestrator revision")?,
            owner,
        })
    }
    fn next(&self, owner: Option<Owner>) -> Result<String> {
        let revision = self
            .revision
            .checked_add(1)
            .context("orchestrator revision overflow")?;
        Ok(match owner {
            None => revision.to_string(),
            Some(owner) => format!(
                "{revision} {} {} {}",
                owner.pane,
                owner.pid,
                owner.kind.as_str()
            ),
        })
    }
}

/// All substituted values are validated numeric identifiers or enum words. Never interpolate
/// task text, names or arbitrary option contents into a tmux format.
fn exchange(pane: &tmux::Pane, expected: &str, next: &str, after: &str) -> String {
    let condition = format!(
        "#{{&&:#{{==:#{{session_id}},{}}},#{{&&:#{{==:#{{pane_pid}},{}}},#{{==:#{{{OPTION}}},{expected}}}}}}}",
        pane.session_id, pane.pid,
    );
    let assign = tmux::command_string(&["set-option", "-t", &pane.session_id, OPTION, next]);
    let success = format!("{assign} ; {after}");
    tmux::command_string(&[
        "if-shell",
        "-F",
        "-t",
        &pane.id,
        &condition,
        &success,
        &tmux::command_string(&["display-message", "-p", "changed"]),
    ])
}

fn write(pane: &tmux::Pane, expected: &str, next: &str) -> Result<bool> {
    let command = exchange(pane, expected, next, "display-message -p applied");
    Ok(tmux::run(&["if-shell", "-F", "1", &command])?.trim() == "applied")
}

pub fn run(action: Action) -> Result<()> {
    let (target, promote) = match &action {
        Action::Set { target } => (target, true),
        Action::Clear { target } => (target, false),
    };
    let index = index::read()?;
    let indexed = index::find(&index, &commands::target_pane(target)?)?;
    let panes = tmux::list_panes()?;
    let detected = detect::detect(&panes)?;
    let live = detected
        .values
        .iter()
        .find(|d| d.pane.id == indexed.pane)
        .context("agent is no longer live or its native identity is unavailable")?;
    if live.pid != indexed.process_id
        || live.kind != indexed.kind
        || live.session_id != indexed.session_id
        || live.pane.session != indexed.session
    {
        bail!("agent identity or project changed; list agents again before reporting its role");
    }
    let record = Record::parse(&live.pane.orchestrator_role)?;
    let owner = if promote {
        Some(Owner::of(indexed))
    } else {
        record.owner.clone().filter(|o| !o.matches(indexed))
    };
    // Even an already ordinary agent records its demotion, invalidating pending inference.
    let next = record.next(owner)?;
    if !write(&live.pane, &live.pane.orchestrator_role, &next)? {
        bail!("project role or placement changed concurrently; list agents again before retrying");
    }
    lifecycle::run(lifecycle::Action::Tick).context("role saved, but could not wake the daemon")?;
    println!(
        "{} {} in {}",
        indexed.name,
        if promote {
            "is the orchestrator"
        } else {
            "has stepped down"
        },
        indexed.session
    );
    Ok(())
}

/// An inference made with this naming job's snapshot can only fill the vacancy it observed.
/// Included in the same guarded command that assigns the name.
pub fn inference(pane: &tmux::Pane, agent: &Agent) -> Result<Option<String>> {
    let record = Record::parse(&pane.orchestrator_role)?;
    if record.owner.is_some() {
        return Ok(None);
    }
    Ok(Some(exchange(
        pane,
        &pane.orchestrator_role,
        &record.next(Some(Owner::of(agent)))?,
        "display-message -p applied",
    )))
}

/// Project assignments are projected from the existing snapshot, costing no idle tmux calls.
/// Clear an ended owner's assignment only when its native scan is healthy. No successor election.
pub fn resolve(agents: &mut [Agent], panes: &[tmux::Pane], faults: &[Issue]) -> Vec<Issue> {
    for agent in agents.iter_mut() {
        agent.orchestrator = false;
    }
    let mut issues = Vec::new();
    let mut projects = HashSet::new();
    for pane in panes {
        if !projects.insert(&pane.session_id) {
            continue;
        }
        let result = (|| -> Result<()> {
            let record = Record::parse(&pane.orchestrator_role)?;
            let Some(owner) = record.owner.as_ref() else {
                return Ok(());
            };
            if let Some(agent) = agents
                .iter_mut()
                .find(|a| a.session == pane.session && owner.matches(a))
            {
                agent.orchestrator = true;
            } else if !faults.iter().any(|i| {
                i.component == format!("scan:{}", owner.kind.as_str())
                    || i.component == format!("advance:{}:{}", owner.kind.as_str(), owner.pane)
            }) {
                // CAS leaves a newer promotion or demotion untouched.
                write(pane, &pane.orchestrator_role, &record.next(None)?)?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            issues.push(Issue::new(
                format!("orchestrator:{}", pane.session_id),
                error,
            ));
        }
    }
    issues
}

/// Role checks and writes join the existing guarded native move. The role follows its agent
/// into a vacant project; a project with a leader requires an explicit role decision first.
pub struct MoveRole {
    pub source_condition: String,
    pub destination_condition: String,
    pub before: Vec<String>,
    pub after: Vec<String>,
}

pub fn moving(agent: &Agent, source: &str, destination: &str) -> Result<MoveRole> {
    let output = tmux::run(&[
        "display-message",
        "-p",
        "-t",
        &format!("{source}:"),
        &format!("#{{{OPTION}}}"),
        ";",
        "display-message",
        "-p",
        "-t",
        &format!("{destination}:"),
        &format!("#{{{OPTION}}}"),
    ])?;
    let values: Vec<_> = output.lines().collect();
    let [raw_source, raw_destination] = values.as_slice() else {
        bail!("unexpected project role snapshot while moving");
    };
    let record = Record::parse(raw_source)?;
    let mut plan = MoveRole {
        source_condition: format!("#{{==:#{{{OPTION}}},{raw_source}}}"),
        destination_condition: "1".into(),
        before: Vec::new(),
        after: Vec::new(),
    };
    if record.owner.as_ref().is_some_and(|o| o.matches(agent)) {
        let dest = Record::parse(raw_destination)?;
        if dest.owner.is_some() {
            bail!(
                "destination project already has an orchestrator; report which agent should step down before moving"
            );
        }
        plan.destination_condition = format!("#{{==:#{{{OPTION}}},{raw_destination}}}");
        plan.before.push(tmux::command_string(&[
            "set-option",
            "-t",
            source,
            OPTION,
            &record.next(None)?,
        ]));
        plan.after.push(tmux::command_string(&[
            "set-option",
            "-t",
            destination,
            OPTION,
            &dest.next(Some(Owner::of(agent)))?,
        ]));
    }
    Ok(plan)
}

/// The shared project order used by lists and direct agent navigation.
pub fn project_order(a: &Agent, b: &Agent) -> std::cmp::Ordering {
    a.session
        .cmp(&b.session)
        .then_with(|| b.orchestrator.cmp(&a.orchestrator))
}

/// A matching orchestrator leads its project even in ranked text results. Other matches keep
/// their relative rank, and filtering never resurrects an orchestrator that did not match.
pub fn pin(agents: &mut [Agent]) {
    for i in 0..agents.len() {
        if agents[i].orchestrator {
            let start = agents[..i]
                .iter()
                .position(|a| a.session == agents[i].session)
                .unwrap_or(i);
            agents[start..=i].rotate_right(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assignments_are_strict_and_revisions_preserve_explicit_vacancies() {
        assert_eq!(Record::parse("").unwrap(), Record::default());
        let ordinary = Record::parse("7").unwrap();
        assert_eq!(ordinary.next(None).unwrap(), "8");
        let owner = Owner {
            pane: "%4".into(),
            pid: 12,
            kind: Kind::Codex,
        };
        let assigned = ordinary.next(Some(owner.clone())).unwrap();
        assert_eq!(Record::parse(&assigned).unwrap().owner, Some(owner));
        for bad in [
            "junk",
            "1 %x 12 codex",
            "1 %4 -1 codex",
            "1 %4 12 fish",
            "1 %4 12 codex extra",
            "1,#{bad}",
        ] {
            assert!(Record::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_successor_process_does_not_inherit_the_role() {
        let mut agent = crate::links::tests::agent("%4");
        agent.process_id = 12;
        let owner = Owner::of(&agent);
        assert!(owner.matches(&agent));
        agent.process_id = 13;
        assert!(!owner.matches(&agent));
    }
}
