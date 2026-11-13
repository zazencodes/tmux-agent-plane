//! Workspaces: named groups of tmux sessions, a layer above tmux. Every session belongs to one,
//! recorded on the session as `@agent-plane-workspace`, and tmux itself still shows every
//! session. The sidebar, the picker, the agent keys and the status bar show the agents of one
//! workspace: the one you are looking from.
//!
//! A session the plane creates joins the workspace of whoever created it. Any other new session
//! joins the workspace you are in, which is that of the client used last.

use crate::diagnostics::Issue;
use crate::index::{self, Agent, Index, Workspace, WorkspaceSession};
use crate::{commands, sessions, tasks, tmux};
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::collections::BTreeMap;

/// Session option naming its workspace.
pub const OPTION: &str = "@agent-plane-workspace";
/// The workspace sessions join before there is any other.
pub const FIRST: &str = "main";
const SEP: char = '\x1f';

#[derive(clap::Subcommand)]
pub enum Action {
    /// List workspaces with their sessions and agents; `*` marks the one you are in
    List {
        #[arg(long)]
        json: bool,
    },
    /// Show a workspace in your tmux client, at the session you used there last
    Switch { name: String },
    /// Put whole tmux sessions, with their agents, in a workspace, starting it if it is new
    Move {
        /// Exact session names or ids
        #[arg(required = true)]
        sessions: Vec<String>,
        /// The workspace they join
        #[arg(long)]
        to: String,
    },
    /// Rename a workspace
    Rename { name: String, new_name: String },
}

pub fn run(action: Action) -> Result<()> {
    match action {
        Action::List { json } => list(json),
        Action::Switch { name } => {
            let (client, session) = switch(&name, None)?;
            println!("{client} shows workspace {name} at session {session}");
            Ok(())
        }
        Action::Move { sessions, to } => {
            for line in move_sessions(&sessions, &to)? {
                println!("{line}");
            }
            Ok(())
        }
        Action::Rename { name, new_name } => {
            rename(&name, &new_name)?;
            println!("workspace {name} is now {new_name}");
            Ok(())
        }
    }
}

/// The rules of session names, so a workspace can always start with a session of its own name.
pub fn validate(name: &str) -> Result<()> {
    if name.is_empty()
        || name.trim() != name
        || name.contains(['.', ':'])
        || name.chars().any(char::is_control)
        || name.chars().count() > 32
    {
        bail!(
            "workspace names are nonempty, at most 32 characters, without dots, colons, control characters or surrounding whitespace: {name:?}"
        );
    }
    Ok(())
}

/// The daemon's part: the workspace you are in, and one for every session and task without one.
pub struct Tracker {
    current: String,
}

pub struct Tick {
    pub workspaces: Vec<Workspace>,
    /// The workspace you are in.
    pub current: String,
    pub issues: Vec<Issue>,
}

impl Tracker {
    pub fn start(previous: Option<&Index>) -> Self {
        Self {
            current: previous.map(|i| i.workspace.clone()).unwrap_or_default(),
        }
    }

    /// Follow the client used last into its workspace, then give each new session the workspace
    /// you were in when it appeared. The snapshot is updated to match.
    pub fn tick(&mut self, snapshot: &mut tmux::Snapshot) -> Tick {
        let last_used = snapshot
            .geometry
            .clients
            .iter()
            .max_by_key(|client| client.activity);
        if let Some(workspace) = last_used.and_then(|client| {
            snapshot
                .sessions
                .iter()
                .find(|s| s.id == client.session && !s.workspace.is_empty())
        }) {
            self.current.clone_from(&workspace.workspace);
        }
        if self.current.is_empty() {
            self.current = FIRST.into();
        }
        let new: Vec<String> = snapshot
            .sessions
            .iter()
            .filter(|s| s.workspace.is_empty() && s.name != tasks::SESSION)
            .map(|s| s.id.clone())
            .collect();
        let commands: Vec<_> = new
            .iter()
            .map(|id| {
                (
                    id.clone(),
                    label_if_unset(id, OPTION, &["-t", id], &self.current),
                )
            })
            .collect();
        let mut issues = Vec::new();
        match tmux::for_each(&commands) {
            Ok(()) => {
                for session in &mut snapshot.sessions {
                    if new.contains(&session.id) {
                        session.workspace.clone_from(&self.current);
                    }
                }
                for pane in &mut snapshot.panes {
                    if new.contains(&pane.session_id) {
                        pane.workspace.clone_from(&self.current);
                    }
                }
            }
            Err(error) => issues.push(Issue::new("workspaces", error)),
        }
        Tick {
            workspaces: views(&snapshot.sessions),
            current: self.current.clone(),
            issues,
        }
    }

    /// Tasks started before workspaces existed join the workspace you are in.
    pub fn label_tasks(&self, tasks: &mut [tasks::Task]) -> Vec<Issue> {
        let new: Vec<String> = tasks
            .iter()
            .filter(|t| t.definition.workspace.is_empty())
            .map(|t| t.pane.clone())
            .collect();
        let commands: Vec<_> = new
            .iter()
            .map(|pane| {
                (
                    pane.clone(),
                    label_if_unset(pane, tasks::WORKSPACE, &["-p", "-t", pane], &self.current),
                )
            })
            .collect();
        if let Err(error) = tmux::for_each(&commands) {
            return vec![Issue::new("workspaces:tasks", error)];
        }
        for task in tasks {
            if new.contains(&task.pane) {
                task.definition.workspace.clone_from(&self.current);
            }
        }
        Vec::new()
    }
}

/// Set `option` only while it is unset, checked inside tmux's command queue: a session's creator
/// may name its workspace between the daemon's snapshot and this command.
fn label_if_unset(target: &str, option: &str, scope: &[&str], workspace: &str) -> Vec<String> {
    let mut set = vec!["set-option"];
    set.extend(scope);
    set.extend([option, workspace]);
    [
        "if-shell",
        "-F",
        "-t",
        target,
        &format!("#{{==:#{{{option}}},}}"),
        &tmux::command_string(&set),
    ]
    .map(String::from)
    .to_vec()
}

/// Every workspace by name, with its sessions by name. The tasks session belongs to none.
fn views(sessions: &[tmux::Session]) -> Vec<Workspace> {
    let mut workspaces: BTreeMap<&str, Vec<WorkspaceSession>> = BTreeMap::new();
    for session in sessions {
        if session.workspace.is_empty() || session.name == tasks::SESSION {
            continue;
        }
        workspaces
            .entry(&session.workspace)
            .or_default()
            .push(WorkspaceSession {
                id: session.id.clone(),
                name: session.name.clone(),
            });
    }
    workspaces
        .into_iter()
        .map(|(name, mut sessions)| {
            sessions.sort_by(|a, b| a.name.cmp(&b.name));
            Workspace {
                name: name.to_owned(),
                sessions,
            }
        })
        .collect()
}

/// The workspace a session or task this command creates joins: the one named, else that of the
/// agent or pane creating it, else the one you are in.
pub fn for_new(named: Option<&str>, creator: Option<&Agent>, index: &Index) -> Result<String> {
    if let Some(name) = named {
        validate(name)?;
        return Ok(name.to_owned());
    }
    if let Some(agent) = creator.filter(|a| !a.workspace.is_empty()) {
        return Ok(agent.workspace.clone());
    }
    // A Codex tool shell may inherit another pane's TMUX_PANE, so its pane says nothing. A
    // caller pane that has closed says nothing either.
    if !commands::codex_caller()
        && let Some(pane) = commands::self_pane()?
    {
        let panes = tmux::run(&[
            "list-panes",
            "-a",
            "-F",
            &format!("#{{pane_id}}{SEP}#{{{OPTION}}}"),
        ])?;
        if let Some(label) = panes
            .lines()
            .filter_map(|line| line.split_once(SEP))
            .find(|(id, _)| *id == pane)
            .map(|(_, label)| label)
            .filter(|label| !label.is_empty())
        {
            return Ok(label.to_owned());
        }
    }
    if index.workspace.is_empty() {
        bail!(
            "the daemon has not published a workspace; restart it with `tmux-agent-plane daemon restart`"
        );
    }
    Ok(index.workspace.clone())
}

/// Put a session in a workspace.
pub fn assign(session_id: &str, workspace: &str) -> Result<()> {
    tmux::run(&["set-option", "-t", session_id, OPTION, workspace]).map(drop)
}

#[derive(Serialize)]
struct Listed<'a> {
    name: &'a str,
    /// You are in it: the client used last shows one of its sessions.
    current: bool,
    sessions: &'a [WorkspaceSession],
    agents: Vec<&'a str>,
}

fn list(json: bool) -> Result<()> {
    let index = index::read()?;
    let listed: Vec<Listed> = index
        .workspaces
        .iter()
        .map(|w| Listed {
            name: &w.name,
            current: w.name == index.workspace,
            sessions: &w.sessions,
            agents: index
                .agents
                .iter()
                .filter(|a| a.workspace == w.name)
                .map(|a| a.name.as_str())
                .collect(),
        })
        .collect();
    if json {
        println!("{}", serde_json::to_string_pretty(&listed)?);
        return Ok(());
    }
    for workspace in listed {
        let sessions: Vec<&str> = workspace.sessions.iter().map(|s| s.name.as_str()).collect();
        println!(
            "{}{}  sessions: {}  agents: {}",
            if workspace.current { "*" } else { " " },
            workspace.name,
            sessions.join(", "),
            if workspace.agents.is_empty() {
                "-".to_owned()
            } else {
                workspace.agents.join(", ")
            }
        );
    }
    Ok(())
}

/// The session of a workspace used last, by tmux's activity time.
fn last_used_session(workspace: &str) -> Result<(String, String)> {
    let rows = tmux::run(&[
        "list-sessions",
        "-F",
        &format!(
            "#{{session_id}}{SEP}#{{session_name}}{SEP}#{{session_activity}}{SEP}#{{{OPTION}}}"
        ),
    ])?;
    let mut best: Option<(u64, String, String)> = None;
    let mut names = Vec::new();
    for line in rows.lines() {
        let [id, name, activity, label] = line.split(SEP).collect::<Vec<_>>()[..] else {
            bail!("unexpected list-sessions row {line:?}");
        };
        if label.is_empty() || name == tasks::SESSION {
            continue;
        }
        if !names.contains(&label) {
            names.push(label);
        }
        let activity: u64 = activity.parse().context("session_activity")?;
        if label == workspace && best.as_ref().is_none_or(|(at, _, _)| activity > *at) {
            best = Some((activity, id.to_owned(), name.to_owned()));
        }
    }
    match best {
        Some((_, id, name)) => Ok((id, name)),
        None => {
            names.sort_unstable();
            bail!(
                "no workspace named {workspace:?}. Workspaces: {}",
                names.join(", ")
            )
        }
    }
}

/// Show a workspace in a client, the one used last unless named. Returns the client and the
/// session it now shows.
pub fn switch(workspace: &str, client: Option<&str>) -> Result<(String, String)> {
    let (id, name) = last_used_session(workspace)?;
    let client = match client {
        Some(client) => client.to_owned(),
        None => {
            tmux::clients()?
                .into_iter()
                .max_by_key(|c| c.activity)
                .context("no tmux client is attached")?
                .name
        }
    };
    tmux::run(&["switch-client", "-c", &client, "-t", &id])?;
    Ok((client, name))
}

/// Put sessions in a workspace. Their windows, panes and clients stay as they are.
pub fn move_sessions(targets: &[String], workspace: &str) -> Result<Vec<String>> {
    validate(workspace)?;
    let snapshot = sessions::Snapshot::load()?;
    let mut moving = Vec::new();
    for target in targets {
        let session = snapshot.find(target)?;
        if session.name == tasks::SESSION {
            bail!(
                "{:?} holds background tasks and belongs to no workspace",
                session.name
            );
        }
        if session.group.is_some() {
            bail!(
                "session {:?} is grouped and shares its windows with other sessions; move refused",
                session.name
            );
        }
        if !moving
            .iter()
            .any(|s: &&sessions::Session| s.id == session.id)
        {
            moving.push(session);
        }
    }
    let commands: Vec<_> = moving
        .iter()
        .map(|s| {
            (
                s.id.clone(),
                ["set-option", "-t", &s.id, OPTION, workspace]
                    .map(String::from)
                    .to_vec(),
            )
        })
        .collect();
    tmux::for_each(&commands)?;
    Ok(moving
        .iter()
        .map(|s| format!("{} {} is in workspace {workspace}", s.name, s.id))
        .collect())
}

/// Rename a workspace: each of its sessions and tasks takes the new name.
pub fn rename(workspace: &str, new_name: &str) -> Result<()> {
    validate(new_name)?;
    let snapshot = sessions::Snapshot::load()?;
    let members: Vec<_> = snapshot
        .sessions
        .iter()
        .filter(|s| s.workspace == workspace && s.name != tasks::SESSION)
        .collect();
    if members.is_empty() {
        bail!("no workspace named {workspace:?}");
    }
    if workspace == new_name {
        return Ok(());
    }
    if snapshot.sessions.iter().any(|s| s.workspace == new_name) {
        bail!(
            "workspace {new_name:?} exists; put sessions in it with `tmux-agent-plane workspace move`"
        );
    }
    let mut commands: Vec<_> = members
        .iter()
        .map(|s| {
            (
                s.id.clone(),
                ["set-option", "-t", &s.id, OPTION, new_name]
                    .map(String::from)
                    .to_vec(),
            )
        })
        .collect();
    for task in index::read()?
        .tasks
        .iter()
        .filter(|t| t.definition.workspace == workspace)
    {
        commands.push((
            task.pane.clone(),
            [
                "set-option",
                "-p",
                "-t",
                &task.pane,
                tasks::WORKSPACE,
                new_name,
            ]
            .map(String::from)
            .to_vec(),
        ));
    }
    tmux::for_each(&commands)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, name: &str, workspace: &str) -> tmux::Session {
        tmux::Session {
            id: id.into(),
            name: name.into(),
            workspace: workspace.into(),
        }
    }

    #[test]
    fn workspace_names_follow_session_names() {
        for name in ["", " a", "a ", "a.b", "a:b", "a\nb", &"x".repeat(33)] {
            assert!(validate(name).is_err(), "{name:?}");
        }
        for name in ["main", "client-x", "two words", "日本語", &"x".repeat(32)] {
            assert!(validate(name).is_ok(), "{name:?}");
        }
    }

    #[test]
    fn views_group_sessions_by_workspace_and_leave_out_tasks_and_new_sessions() {
        let views = views(&[
            session("$2", "web", "main"),
            session("$1", "api", "main"),
            session("$3", "client", "client-x"),
            session("$4", tasks::SESSION, "main"),
            session("$5", "fresh", ""),
        ]);
        assert_eq!(
            views
                .iter()
                .map(|w| (
                    w.name.as_str(),
                    w.sessions
                        .iter()
                        .map(|s| s.name.as_str())
                        .collect::<Vec<_>>()
                ))
                .collect::<Vec<_>>(),
            [("client-x", vec!["client"]), ("main", vec!["api", "web"])]
        );
    }

    #[test]
    fn labels_are_set_only_while_unset_inside_tmux() {
        let command = label_if_unset("$3", OPTION, &["-t", "$3"], "it's");
        assert_eq!(
            command,
            [
                "if-shell",
                "-F",
                "-t",
                "$3",
                "#{==:#{@agent-plane-workspace},}",
                r"'set-option' '-t' '$3' '@agent-plane-workspace' 'it'\''s'"
            ]
        );
    }

    #[test]
    fn a_named_workspace_wins_and_an_agent_creator_comes_next() {
        let index = Index {
            workspace: "main".into(),
            ..Index::default()
        };
        assert_eq!(for_new(Some("client-x"), None, &index).unwrap(), "client-x");
        assert!(for_new(Some("bad.name"), None, &index).is_err());
        let agent: Agent = serde_json::from_value(serde_json::json!({
            "name": "lead", "pane": "%1", "kind": "codex", "state": "idle", "detail": null,
            "title": "", "location": "s:1.0", "session": "s", "workspace": "research",
            "window": "@1", "cwd": "/tmp", "parent": null, "session_id": null,
            "state_since": 0, "completions": 0, "visible": false
        }))
        .unwrap();
        assert_eq!(for_new(None, Some(&agent), &index).unwrap(), "research");
    }
}
