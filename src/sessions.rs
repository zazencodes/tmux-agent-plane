//! A tmux session is a project. Resolve exact destinations and move physical panes.
use crate::index::{self, Agent};
use crate::tmux::command_string as command;
use crate::{detect, navigation, tasks, tmux};
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::path::PathBuf;

const SEP: char = '\x1f';
const SNAPSHOT: &str = "#{session_id}\x1f#{session_name}\x1f#{session_attached}\x1f#{session_group}\x1f#{window_id}\x1f#{pane_id}\x1f#{@agent-plane-sidebar}\x1f#{@agent-plane-workspace}";

#[derive(clap::Subcommand)]
pub enum Action {
    /// List every project, including sessions without tracked agents
    List {
        #[arg(long)]
        json: bool,
    },
    /// Create a detached project with an interactive shell
    Create {
        /// Its name (default: its folder's, with -2, -3... when a session has that name)
        name: Option<String>,
        /// Working directory (default: current directory)
        #[arg(long)]
        cwd: Option<PathBuf>,
        /// The workspace it joins (default: the caller's, else the one you are in)
        #[arg(long)]
        workspace: Option<String>,
    },
}

pub fn run(action: Action) -> Result<()> {
    match action {
        Action::List { json } => list(json),
        Action::Create {
            name,
            cwd,
            workspace,
        } => create(name.as_deref(), cwd, workspace.as_deref()),
    }
}

#[derive(Debug, Serialize)]
pub struct Session {
    pub id: String,
    pub name: String,
    /// Empty only for the tasks session and a session too new to have one.
    pub workspace: String,
    attached: u32,
    pub(crate) group: Option<String>,
    /// Physical window ids make links between projects explicit.
    pub(crate) windows: Vec<String>,
    agents: Vec<String>,
}

struct PaneLink {
    id: String,
    window: String,
    session: String,
    sidebar: bool,
}

pub(crate) struct Snapshot {
    pub(crate) sessions: Vec<Session>,
    panes: Vec<PaneLink>,
}

impl Snapshot {
    pub(crate) fn load() -> Result<Self> {
        Self::parse(&tmux::run(&["list-panes", "-a", "-F", SNAPSHOT])?)
    }

    fn parse(output: &str) -> Result<Self> {
        let mut snapshot = Self {
            sessions: Vec::new(),
            panes: Vec::new(),
        };
        for line in output.lines() {
            let fields: Vec<_> = line.split(SEP).collect();
            if fields.len() != 8 {
                bail!("unexpected session/pane snapshot row: {line:?}");
            }
            let [id, name, attached, group, window, pane, sidebar, workspace] = fields[..] else {
                unreachable!()
            };
            let session = match snapshot.sessions.iter().position(|s| s.id == id) {
                Some(i) => &mut snapshot.sessions[i],
                None => {
                    snapshot.sessions.push(Session {
                        id: id.into(),
                        name: name.into(),
                        workspace: workspace.into(),
                        attached: attached.parse().context("session_attached")?,
                        group: Some(group.into()).filter(|g: &String| !g.is_empty()),
                        windows: Vec::new(),
                        agents: Vec::new(),
                    });
                    snapshot.sessions.last_mut().expect("just inserted")
                }
            };
            if !session.windows.iter().any(|w| w == window) {
                session.windows.push(window.into());
            }
            snapshot.panes.push(PaneLink {
                id: pane.into(),
                window: window.into(),
                session: id.into(),
                sidebar: sidebar == "1",
            });
        }
        snapshot.sessions.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(snapshot)
    }

    pub(crate) fn find(&self, target: &str) -> Result<&Session> {
        let matches: Vec<_> = self
            .sessions
            .iter()
            .filter(|s| s.name == target || s.id == target)
            .collect();
        match matches.as_slice() {
            [session] => Ok(session),
            [] => bail!(
                "no session with exact name or id {target:?}; see `tmux-agent-plane sessions list`"
            ),
            _ => bail!("ambiguous session name/id {target:?}; use a unique session id"),
        }
    }
}

fn list(json: bool) -> Result<()> {
    let index = index::read()?;
    // The tasks session holds background tasks, never a project.
    let sessions: Vec<Session> = inventory(&index.agents)?
        .into_iter()
        .filter(|s| s.name != tasks::SESSION)
        .collect();
    if json {
        println!("{}", serde_json::to_string_pretty(&sessions)?);
    } else {
        for session in sessions {
            println!(
                "{} {}  workspace {}  {} windows  {} clients  agents: {}{}",
                session.name,
                session.id,
                session.workspace,
                session.windows.len(),
                session.attached,
                session.agents.join(", "),
                session
                    .group
                    .map_or(String::new(), |g| format!("  group: {g}"))
            );
        }
    }
    Ok(())
}

pub fn inventory(agents: &[Agent]) -> Result<Vec<Session>> {
    let mut snapshot = Snapshot::load()?;
    for session in &mut snapshot.sessions {
        session.agents = agents
            .iter()
            .filter(|a| {
                snapshot
                    .panes
                    .iter()
                    .any(|p| p.session == session.id && p.id == a.pane)
            })
            .map(|a| a.name.clone())
            .collect();
    }
    Ok(snapshot.sessions)
}

pub fn validate_name(name: &str) -> Result<()> {
    // tmux silently replaces dots and colons. Reject them so the requested name
    // is exactly the one that gets created, and keep list/format output intact.
    if name.is_empty()
        || name.trim() != name
        || name.contains(['.', ':'])
        || name.chars().any(char::is_control)
    {
        bail!(
            "session name must be nonempty, without dots, colons, control characters or surrounding whitespace"
        );
    }
    if name == tasks::SESSION {
        bail!("{name:?} is the background tasks session; choose another project name");
    }
    Ok(())
}

fn create(name: Option<&str>, cwd: Option<PathBuf>, workspace: Option<&str>) -> Result<()> {
    let workspace = crate::workspaces::for_new(workspace, None, &index::read()?)?;
    let (id, name) = match name {
        Some(name) => (create_project(name, cwd, &workspace)?, name.to_owned()),
        None => create_folder_project(cwd, &workspace)?,
    };
    println!("{name} {id} {workspace}");
    Ok(())
}

/// A new project named after its folder: the folder's name, or with -2, -3... when a session
/// already has it. tmux session names are unique, and the same folder can be a project in several
/// workspaces. Returns its id and name.
pub fn create_folder_project(cwd: Option<PathBuf>, workspace: &str) -> Result<(String, String)> {
    let cwd = match cwd {
        Some(cwd) => cwd,
        None => std::env::current_dir()?,
    };
    let folder = crate::projects::project_name(cwd.to_str().context("folder is not UTF-8")?)
        .with_context(|| format!("{} has no name to give a project; name it", cwd.display()))?;
    let snapshot = Snapshot::load()?;
    let name = candidates(&folder)
        .find(|name| !snapshot.sessions.iter().any(|s| &s.name == name))
        .expect("names never run out");
    Ok((create_project(&name, Some(cwd), workspace)?, name))
}

/// A folder's project names, in order: its name, then with -2, -3...
fn candidates(folder: &str) -> impl Iterator<Item = String> + '_ {
    std::iter::once(folder.to_owned()).chain((2..).map(move |n| format!("{folder}-{n}")))
}

/// A detached session with a login shell, in a workspace.
pub fn create_project(name: &str, cwd: Option<PathBuf>, workspace: &str) -> Result<String> {
    validate_name(name)?;
    let cwd = match cwd {
        Some(cwd) => cwd,
        None => std::env::current_dir()?,
    };
    let cwd = cwd.canonicalize()?;
    if !cwd.is_dir() {
        bail!("session cwd is not a directory: {}", cwd.display());
    }
    let shell = tmux::default_shell(None)?;
    let created = tmux::run(&[
        "new-session",
        "-d",
        "-P",
        "-F",
        "#{session_id}",
        "-s",
        name,
        "-c",
        cwd.to_str().context("session cwd is not UTF-8")?,
        &shell,
        "-il",
    ])?;
    let id = created.trim().to_owned();
    crate::workspaces::assign(&id, workspace)?;
    Ok(id)
}

/// Agents go to projects; the tasks session holds only tasks.
fn project(session: &Session) -> Result<()> {
    if session.name == tasks::SESSION {
        bail!(
            "{:?} holds background tasks, not agents; choose a project session",
            session.name
        );
    }
    independent(session)
}

fn independent(session: &Session) -> Result<()> {
    if session.group.is_some() {
        bail!(
            "session {:?} is grouped and shares its windows; choose an independent project session",
            session.name
        );
    }
    Ok(())
}

/// An explicit existing destination is always resolved to its immutable id.
pub fn destination(target: &str) -> Result<String> {
    let snapshot = Snapshot::load()?;
    let session = snapshot.find(target)?;
    project(session)?;
    Ok(session.id.clone())
}

struct MovePlan<'a> {
    source: &'a Session,
    destination: &'a Session,
    window: &'a str,
    pane_count: usize,
    final_window: bool,
}

/// `following` counts the clients that switch to the destination with the agent.
fn plan_move<'a>(
    snapshot: &'a Snapshot,
    pane: &str,
    target: &str,
    following: usize,
) -> Result<MovePlan<'a>> {
    let destination = snapshot.find(target)?;
    project(destination)?;
    let links: Vec<_> = snapshot.panes.iter().filter(|p| p.id == pane).collect();
    let [link] = links.as_slice() else {
        bail!("agent pane is missing or its window is linked into multiple sessions; move refused");
    };
    if link.sidebar {
        bail!("cannot move a sidebar as an agent");
    }
    let source = snapshot.find(&link.session)?;
    independent(source)?;
    if source.id == destination.id {
        bail!("agent is already in session {:?}", source.name);
    }
    let window_panes: Vec<_> = snapshot
        .panes
        .iter()
        .filter(|p| p.window == link.window)
        .collect();
    // The plugin closes a lone sidebar immediately. Account for that even if
    // the hook has not yet run; never end any client's attached session.
    let final_window =
        source.windows.len() == 1 && window_panes.iter().all(|p| p.id == pane || p.sidebar);
    if final_window && source.attached as usize != following {
        bail!(
            "move would end attached session {:?}; leave another window or content pane in that session",
            source.name
        );
    }
    Ok(MovePlan {
        source,
        destination,
        window: &link.window,
        pane_count: window_panes.len(),
        final_window,
    })
}

pub fn move_agent(agent: &Agent, target: &str) -> Result<()> {
    // Refresh the native identity before moving an indexed name. A state/turn
    // change is fine, but a different agent in the same pane requires a new list.
    let panes = tmux::list_panes()?;
    let detected = detect::detect(&panes)?;
    let live = detected
        .values
        .iter()
        .find(|d| d.pane.id == agent.pane)
        .context("agent is no longer live or its native identity is unavailable; move refused")?;
    let name = if live.pane.name.is_empty() {
        format!(
            "{}-{}",
            live.kind.as_str(),
            live.pane.id.trim_start_matches('%')
        )
    } else {
        live.pane.name.clone()
    };
    if live.kind != agent.kind
        || live.session_id != agent.session_id
        || live.completion_epoch != agent.completion_epoch
        || name != agent.name
        || live.pane.session != agent.session
        || live.pane.window != agent.window
    {
        bail!("agent identity/name/placement changed; list agents again before moving");
    }
    let location = relocate(agent, live.pane.pid, target, &[])?;
    println!("{} {} {location}", agent.name, agent.pane);
    Ok(())
}

/// Move a verified agent and mark it placed, in one tmux command, so nothing shows in between.
/// Followers switch first because they may be the last clients of a session the move ends.
fn relocate(agent: &Agent, pid: i32, target: &str, followers: &[String]) -> Result<String> {
    let snapshot = Snapshot::load()?;
    let plan = plan_move(&snapshot, &agent.pane, target, followers.len())?;
    if plan.window != agent.window {
        bail!("agent window changed while planning move; retry after listing agents");
    }
    let output_format =
        "#{pane_id}\x1f#{session_id}\x1f#{session_name}:#{window_index}.#{pane_index}";
    let destination_window = format!("{}:", plan.destination.id);
    let switch = |target: &str| -> Vec<String> {
        followers
            .iter()
            .map(|client| command(&["switch-client", "-c", client, "-t", target]))
            .collect()
    };
    let mut moving = switch(&destination_window);
    moving.push(command(&[
        "break-pane",
        "-d",
        "-P",
        "-F",
        output_format,
        "-s",
        &agent.pane,
        "-t",
        &destination_window,
        "-n",
        &agent.name,
    ]));
    moving.push(command(&[
        "set-option",
        "-p",
        "-t",
        &agent.pane,
        tmux::PLACED_OPTION,
        "1",
    ]));
    moving.extend(switch(&agent.pane));
    let moving = moving.join(" ; ");
    let refused = command(&["display-message", "-p", "move-refused"]);
    // Recheck tmux topology inside its command queue. A concurrent link, group,
    // attachment or pane/window removal must not broaden the move's effects.
    let mut checks = vec![
        format!("#{{==:#{{window_id}},{}}}", plan.window),
        format!("#{{==:#{{session_id}},{}}}", plan.source.id),
        format!("#{{==:#{{pane_pid}},{pid}}}"),
        format!("#{{==:#{{window_panes}},{}}}", plan.pane_count),
        format!("#{{==:#{{session_windows}},{}}}", plan.source.windows.len()),
        "#{==:#{window_linked_sessions},1}".into(),
        "#{==:#{session_grouped},0}".into(),
    ];
    if plan.final_window {
        checks.push(format!("#{{==:#{{session_attached}},{}}}", followers.len()));
    }
    let condition = checks
        .into_iter()
        .reduce(|a, b| format!("#{{&&:{a},{b}}}"))
        .expect("nonempty checks");
    let guarded = command(&[
        "if-shell",
        "-F",
        "-t",
        &agent.pane,
        &condition,
        &moving,
        &refused,
    ]);
    let result = tmux::run(&[
        "if-shell",
        "-F",
        "-t",
        &plan.destination.id,
        "#{==:#{session_grouped},0}",
        &guarded,
        &refused,
    ])?;
    let fields: Vec<_> = result.trim_end().split(SEP).collect();
    if fields.len() != 3 || fields[0] != agent.pane || fields[1] != plan.destination.id {
        bail!(
            "tmux topology changed while moving; move refused ({})",
            result.trim()
        );
    }
    Ok(fields[2].to_owned())
}

enum Placement<'a> {
    Stay,
    Existing(&'a Session),
    New(String),
}

/// The folder's project in the agent's workspace is the first of its names (`api`, `api-2`...)
/// that is in that workspace; without one, a new session takes the first name nobody uses, so a
/// project in another workspace is never joined. A window shared with other work, shared sessions
/// and a client already busy in the project keep the agent where it started. Those are the
/// user's arrangement, not faults.
fn plan_placement<'a>(
    snapshot: &'a Snapshot,
    pane: &str,
    project: &str,
    followers: usize,
) -> Result<Placement<'a>> {
    let links: Vec<_> = snapshot.panes.iter().filter(|p| p.id == pane).collect();
    let [link] = links.as_slice() else {
        return Ok(Placement::Stay);
    };
    if project == tasks::SESSION {
        return Ok(Placement::Stay);
    }
    let shared_window = snapshot
        .panes
        .iter()
        .any(|p| p.window == link.window && p.id != pane && !p.sidebar);
    let source = snapshot.find(&link.session)?;
    if shared_window || source.group.is_some() {
        return Ok(Placement::Stay);
    }
    for name in candidates(project) {
        return Ok(match snapshot.sessions.iter().find(|s| s.name == name) {
            Some(s) if s.workspace != source.workspace => continue,
            Some(s) if s.id == source.id => Placement::Stay,
            Some(s) if s.group.is_some() || (followers > 0 && s.attached > 0) => Placement::Stay,
            Some(s) => Placement::Existing(s),
            None => Placement::New(name),
        });
    }
    unreachable!("names never run out")
}

/// Put an agent in its project, taking along every client viewing it. Returns its new location,
/// or nothing when it stays.
pub fn place(agent: &Agent, pid: i32, project: &str) -> Result<Option<String>> {
    let snapshot = Snapshot::load()?;
    let mut followers = Vec::new();
    for line in tmux::run(&["list-clients", "-F", navigation::CLIENT_FORMAT])?.lines() {
        let client = navigation::parse_client(line)?;
        if client.window == agent.window {
            followers.push(client.name);
        }
    }
    let destination = match plan_placement(&snapshot, &agent.pane, project, followers.len())? {
        Placement::Stay => return Ok(None),
        Placement::Existing(session) => session.id.clone(),
        Placement::New(name) => {
            create_project(&name, Some(agent.cwd.clone().into()), &agent.workspace)?
        }
    };
    relocate(agent, pid, &destination, &followers).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(
        id: &str,
        name: &str,
        attached: u32,
        group: &str,
        window: &str,
        pane: &str,
        sidebar: bool,
    ) -> String {
        in_workspace(id, name, attached, group, window, pane, sidebar, "main")
    }

    #[allow(clippy::too_many_arguments)]
    fn in_workspace(
        id: &str,
        name: &str,
        attached: u32,
        group: &str,
        window: &str,
        pane: &str,
        sidebar: bool,
        workspace: &str,
    ) -> String {
        format!(
            "{id}\x1f{name}\x1f{attached}\x1f{group}\x1f{window}\x1f{pane}\x1f{}\x1f{workspace}\n",
            u8::from(sidebar)
        )
    }

    fn base() -> String {
        row("$0", "work", 0, "", "@0", "%0", false)
            + &row("$1", "work-api", 0, "", "@1", "%1", false)
    }

    #[test]
    fn exact_destinations_and_shared_window_inventory() {
        let snapshot =
            Snapshot::parse(&(base() + &row("$2", "alias", 1, "", "@0", "%0", false))).unwrap();
        assert_eq!(snapshot.find("work").unwrap().id, "$0");
        assert_eq!(snapshot.find("$1").unwrap().name, "work-api");
        assert!(snapshot.find("wor").is_err());
        assert_eq!(snapshot.find("alias").unwrap().windows, ["@0"]);
        assert_eq!(snapshot.find("alias").unwrap().attached, 1);
        assert!(Snapshot::parse("bad row").is_err());
    }

    #[test]
    fn moving_only_the_target_leaves_mixed_window_and_attached_session() {
        let snapshot = Snapshot::parse(
            &(row("$0", "work", 2, "", "@0", "%0", false)
                + &row("$0", "work", 2, "", "@0", "%2", false)
                + &row("$0", "work", 2, "", "@0", "%3", true)
                + &row("$1", "api", 1, "", "@1", "%1", false)),
        )
        .unwrap();
        let plan = plan_move(&snapshot, "%0", "api", 0).unwrap();
        assert_eq!(plan.pane_count, 3);
        assert!(!plan.final_window);
        assert_eq!(plan.destination.id, "$1");
        assert!(plan_move(&snapshot, "%0", "work", 0).is_err());
        assert!(plan_move(&snapshot, "%3", "api", 0).is_err());
    }

    #[test]
    fn final_attached_session_accounts_for_sidebar_cleanup() {
        for sidebar in [false, true] {
            let mut output = row("$0", "work", 1, "", "@0", "%0", false)
                + &row("$1", "api", 0, "", "@1", "%1", false);
            if sidebar {
                output += &row("$0", "work", 1, "", "@0", "%2", true);
            }
            let snapshot = Snapshot::parse(&output).unwrap();
            assert!(
                plan_move(&snapshot, "%0", "api", 0)
                    .err()
                    .unwrap()
                    .to_string()
                    .contains("attached session")
            );
            output += &row("$0", "work", 1, "", "@2", "%4", false);
            assert!(
                !plan_move(&Snapshot::parse(&output).unwrap(), "%0", "api", 0)
                    .unwrap()
                    .final_window
            );
        }
        let snapshot = Snapshot::parse(&base()).unwrap();
        assert!(
            plan_move(&snapshot, "%0", "work-api", 0)
                .unwrap()
                .final_window
        );
    }

    #[test]
    fn linked_windows_and_grouped_projects_are_refused() {
        let linked =
            Snapshot::parse(&(base() + &row("$2", "alias", 0, "", "@0", "%0", false))).unwrap();
        assert!(
            plan_move(&linked, "%0", "work-api", 0)
                .err()
                .unwrap()
                .to_string()
                .contains("linked")
        );
        for (source_group, destination_group) in [("team", ""), ("", "team")] {
            let snapshot = Snapshot::parse(
                &(row("$0", "work", 0, source_group, "@0", "%0", false)
                    + &row("$1", "api", 0, destination_group, "@1", "%1", false)),
            )
            .unwrap();
            assert!(
                plan_move(&snapshot, "%0", "api", 0)
                    .err()
                    .unwrap()
                    .to_string()
                    .contains("grouped")
            );
        }
    }

    #[test]
    fn a_followed_agent_may_end_its_session_but_not_with_a_client_left_behind() {
        let snapshot = Snapshot::parse(
            &(row("$0", "work", 1, "", "@0", "%0", false)
                + &row("$1", "api", 0, "", "@1", "%1", false)),
        )
        .unwrap();
        assert!(plan_move(&snapshot, "%0", "api", 1).unwrap().final_window);
        assert!(plan_move(&snapshot, "%0", "api", 0).is_err());
    }

    #[test]
    fn the_tasks_session_is_never_a_project() {
        let rows = base() + &row("$2", tasks::SESSION, 0, "", "@2", "%2", false);
        let snapshot = Snapshot::parse(&rows).unwrap();
        assert!(project(snapshot.find(tasks::SESSION).unwrap()).is_err());
        assert!(project(snapshot.find("work").unwrap()).is_ok());
        assert!(plan_move(&snapshot, "%0", tasks::SESSION, 0).is_err());
        assert!(matches!(
            plan_placement(&snapshot, "%0", tasks::SESSION, 0).unwrap(),
            Placement::Stay
        ));
        assert!(validate_name(tasks::SESSION).is_err());
    }

    #[test]
    fn placement_leaves_shared_arrangements_alone() {
        fn stays(rows: String, followers: usize) -> bool {
            let snapshot = Snapshot::parse(&rows).unwrap();
            matches!(
                plan_placement(&snapshot, "%0", "api", followers).unwrap(),
                Placement::Stay
            )
        }
        let agent = || row("$0", "work", 1, "", "@0", "%0", false);
        let api = |attached, group| row("$1", "api", attached, group, "@1", "%1", false);
        assert!(!stays(agent() + &api(0, ""), 1));
        assert!(!stays(agent(), 1));
        // A sidebar beside the agent does not hold it back; a shell does.
        assert!(!stays(
            agent() + &row("$0", "work", 1, "", "@0", "%2", true) + &api(0, ""),
            1
        ));
        assert!(stays(
            agent() + &row("$0", "work", 1, "", "@0", "%2", false) + &api(0, ""),
            1
        ));
        // Following into a project another client is using would change that client's window.
        assert!(stays(agent() + &api(1, ""), 1));
        assert!(!stays(agent() + &api(1, ""), 0));
        assert!(stays(agent() + &api(0, "team"), 0));
        assert!(stays(
            row("$0", "work", 1, "team", "@0", "%0", false) + &api(0, ""),
            1
        ));
        assert!(stays(
            agent() + &row("$2", "alias", 0, "", "@0", "%0", false),
            1
        ));
    }

    #[test]
    fn a_folder_project_is_found_or_made_in_the_agents_own_workspace() {
        let plan = |rows: String| -> String {
            let snapshot = Snapshot::parse(&rows).unwrap();
            match plan_placement(&snapshot, "%0", "api", 0).unwrap() {
                Placement::Stay => "stay".into(),
                Placement::Existing(s) => format!("join {}", s.name),
                Placement::New(name) => format!("new {name}"),
            }
        };
        let agent = || row("$0", "work", 0, "", "@0", "%0", false);
        let elsewhere = |id, name| in_workspace(id, name, 0, "", "@9", "%9", false, "client-x");
        // api is another workspace's project: this one gets api-2, then joins it.
        assert_eq!(plan(agent() + &elsewhere("$1", "api")), "new api-2");
        assert_eq!(
            plan(agent() + &elsewhere("$1", "api") + &row("$2", "api-2", 0, "", "@2", "%2", false)),
            "join api-2"
        );
        assert_eq!(
            plan(agent() + &elsewhere("$1", "api") + &elsewhere("$2", "api-2")),
            "new api-3"
        );
        // Already in its folder's project, under either name.
        assert_eq!(plan(row("$0", "api", 0, "", "@0", "%0", false)), "stay");
        assert_eq!(
            plan(elsewhere("$1", "api") + &row("$0", "api-2", 0, "", "@0", "%0", false)),
            "stay"
        );
    }

    #[test]
    fn folder_names_count_up_from_the_bare_name() {
        assert_eq!(
            candidates("notes").take(3).collect::<Vec<_>>(),
            ["notes", "notes-2", "notes-3"]
        );
    }

    #[test]
    fn session_names_are_never_silently_rewritten() {
        for name in ["", " spaced", "spaced ", "a.b", "a:b", "a\nb", "a\x1fb"] {
            assert!(validate_name(name).is_err(), "{name:?}");
        }
        for name in ["api", "work-api_2", "two words", "日本語", "quote'$value"] {
            assert!(validate_name(name).is_ok(), "{name:?}");
        }
    }
}
