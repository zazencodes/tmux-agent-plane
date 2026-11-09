//! Direct, client-scoped agent navigation, in the sidebar's published order.

use crate::index::{self, Agent, Index};
use crate::tmux;
use anyhow::{Context, Result, bail};

#[derive(Clone, Copy, clap::ValueEnum)]
pub enum Direction {
    Next,
    Previous,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Client {
    pub name: String,
    pub pid: String,
    pub session: String,
    pub window: String,
    pub pane: String,
}

pub(crate) const CLIENT_FORMAT: &str =
    "#{client_name}\x1f#{client_pid}\x1f#{session_id}\x1f#{window_id}\x1f#{pane_id}";

pub(crate) fn parse_client(line: &str) -> Result<Client> {
    let fields: Vec<_> = line.split('\x1f').collect();
    if fields.len() != 5 {
        bail!("unexpected client context: {line:?}");
    }
    Ok(Client {
        name: fields[0].into(),
        pid: fields[1].into(),
        session: fields[2].into(),
        window: fields[3].into(),
        pane: fields[4].into(),
    })
}

/// Sessions share their current window, and linked windows share their active pane.
/// Explicit -c alone does not isolate those changes. Refuse to disturb another client.
pub(crate) fn guard_destination(
    clients: &[Client],
    client: &Client,
    session: &str,
    window: &str,
) -> Result<()> {
    if clients.iter().any(|other| {
        other.name != client.name && (other.session == session || other.window == window)
    }) {
        bail!("target shared with another client; use a separate session");
    }
    Ok(())
}

struct Pane {
    id: String,
    window: String,
    session: String,
    session_name: String,
}

struct ContextSnapshot {
    clients: Vec<Client>,
    panes: Vec<Pane>,
}

impl ContextSnapshot {
    fn parse(output: &str) -> Result<Self> {
        let mut clients = Vec::new();
        let mut panes = Vec::new();
        for line in output.lines() {
            if let Some(row) = line.strip_prefix("pane\x1f") {
                let fields: Vec<_> = row.split('\x1f').collect();
                if fields.len() != 4 {
                    bail!("unexpected pane context: {line:?}");
                }
                panes.push(Pane {
                    id: fields[0].into(),
                    window: fields[1].into(),
                    session: fields[2].into(),
                    session_name: fields[3].into(),
                });
            } else if let Some(row) = line.strip_prefix("client\x1f") {
                clients.push(parse_client(row)?);
            } else {
                bail!("unexpected navigation context: {line:?}");
            }
        }
        Ok(Self { clients, panes })
    }

    fn client(&self, name: &str) -> Result<&Client> {
        self.clients
            .iter()
            .find(|client| client.name == name)
            .with_context(|| format!("tmux client {name:?} is no longer attached"))
    }

    fn pane(&self, agent: &Agent) -> Result<&Pane> {
        self.panes
            .iter()
            .find(|pane| {
                pane.id == agent.pane
                    && pane.window == agent.window
                    && pane.session_name == agent.session
            })
            .with_context(|| {
                format!(
                    "{} moved or exited; wait for the agent index to update",
                    agent.name
                )
            })
    }
}

fn destination<'a>(index: &'a Index, current: &str, direction: Direction) -> Result<&'a Agent> {
    let mut agents: Vec<_> = index
        .order
        .iter()
        .filter_map(|pane| index.agents.iter().find(|agent| &agent.pane == pane))
        .collect();
    // The sidebar keeps session headings alphabetical while latching order within them.
    agents.sort_by(|a, b| a.session.cmp(&b.session));
    if agents.is_empty() {
        bail!("no agents available in the published sidebar order");
    }
    let position = agents.iter().position(|agent| agent.pane == current);
    let selected = match (position, direction) {
        (Some(position), Direction::Next) => (position + 1) % agents.len(),
        (Some(position), Direction::Previous) => (position + agents.len() - 1) % agents.len(),
        (None, Direction::Next) => 0,
        (None, Direction::Previous) => agents.len() - 1,
    };
    Ok(agents[selected])
}

/// Only explicit client identity supplied by a tmux binding is accepted. The live pane,
/// rather than the sampled last_agent field, makes rapid repeated keys advance correctly.
pub fn cycle(direction: Direction, name: &str) -> Result<()> {
    let index = index::read()?;
    let context = ContextSnapshot::parse(&tmux::run(&[
        "list-clients",
        "-F",
        &format!("client\x1f{CLIENT_FORMAT}"),
        ";",
        "list-panes",
        "-a",
        "-F",
        "pane\x1f#{pane_id}\x1f#{window_id}\x1f#{session_id}\x1f#{session_name}",
    ])?)?;
    let client = context.client(name)?;
    let agent = destination(&index, &client.pane, direction)?;
    let pane = context.pane(agent)?;
    if client.pane == pane.id {
        return Ok(());
    }
    guard_destination(&context.clients, client, &pane.session, &pane.window)?;
    tmux::run(&[
        "switch-client",
        "-c",
        &client.name,
        "-t",
        &format!("{}:{}.{}", pane.session, pane.window, pane.id),
    ])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{Kind, State};

    fn index() -> Index {
        let agents = ["%1", "%2", "%3"].map(|pane| Agent {
            name: pane.into(),
            pane: pane.into(),
            kind: Kind::Pi,
            state: State::Working,
            detail: None,
            title: String::new(),
            location: String::new(),
            session: if pane == "%3" { "alpha" } else { "zeta" }.into(),
            window: "@1".into(),
            cwd: "/tmp".into(),
            parent: None,
            session_id: None,
            state_since: 0,
            completions: 0,
            reported_completions: None,
            completion_epoch: None,
            visible: false,
        });
        Index {
            updated_at: 0,
            agents: agents.into(),
            recent_event: None,
            windows: Vec::new(),
            order: ["%2", "%gone", "%1", "%3"].map(String::from).into(),
            last_agent: Some("%1".into()),
            tasks: Vec::new(),
            pending_restore: 0,
            global_keys: Default::default(),
        }
    }

    #[test]
    fn follows_sidebar_order_across_sessions_and_wraps() {
        let index = index();
        for (current, next, previous) in
            [("%3", "%2", "%1"), ("%2", "%1", "%3"), ("%1", "%3", "%2")]
        {
            assert_eq!(
                destination(&index, current, Direction::Next).unwrap().pane,
                next
            );
            assert_eq!(
                destination(&index, current, Direction::Previous)
                    .unwrap()
                    .pane,
                previous
            );
        }
        assert_eq!(
            destination(&index, "%shell", Direction::Next).unwrap().pane,
            "%3"
        );
        assert_eq!(
            destination(&index, "%sidebar", Direction::Previous)
                .unwrap()
                .pane,
            "%1"
        );
    }

    #[test]
    fn empty_and_single_agent_rosters_are_explicit() {
        let mut index = index();
        index.order = vec!["%1".into()];
        for direction in [Direction::Next, Direction::Previous] {
            assert_eq!(destination(&index, "%1", direction).unwrap().pane, "%1");
        }
        index.agents.clear();
        assert!(destination(&index, "%1", Direction::Next).is_err());
    }

    #[test]
    fn live_context_resolves_exact_client_and_linked_session() {
        let context = ContextSnapshot::parse(concat!(
            "client\x1f/dev/tty1\x1f10\x1f$1\x1f@1\x1f%1\n",
            "pane\x1f%1\x1f@1\x1f$1\x1fother\n",
            "pane\x1f%1\x1f@1\x1f$2\x1fzeta\n"
        ))
        .unwrap();
        assert_eq!(context.client("/dev/tty1").unwrap().pane, "%1");
        assert!(context.client("tty1").is_err());
        let mut agent = index().agents.remove(0);
        assert_eq!(context.pane(&agent).unwrap().session, "$2");
        agent.window = "@moved".into();
        assert!(context.pane(&agent).is_err());
        assert!(ContextSnapshot::parse("pane\x1f%1\n").is_err());
        assert!(ContextSnapshot::parse("not a client\n").is_err());
    }
}
