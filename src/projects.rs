//! An agent started by hand joins the session named after its folder as soon as it is seen.
use crate::diagnostics::{self, Issue};
use crate::index::Agent;
use crate::{sessions, tmux};
use anyhow::{Context, Result, anyhow};
use std::path::Path;

/// tmux would rewrite dots and colons in a session name, so spell them the one way here.
fn project_name(cwd: &str) -> Option<String> {
    let folder = Path::new(cwd).file_name()?.to_str()?;
    let name = folder.replace(['.', ':'], "-");
    let name = name.trim_matches(['-', ' ']);
    (!name.is_empty()).then(|| name.to_owned())
}

#[derive(Default)]
pub struct Placer {
    started: bool,
    /// A failed placement is not retried; its fault lasts while that pane does.
    failure: Option<(String, String)>,
}

impl Placer {
    pub fn tick(&mut self, agents: &[Agent], panes: &[tmux::Pane]) -> Result<Vec<Issue>> {
        for agent in agents {
            let pane = panes
                .iter()
                .find(|p| p.id == agent.pane)
                .context("agent pane missing from its own snapshot")?;
            if pane.placed {
                continue;
            }
            // Agents already running when the daemon starts stay where the user has them.
            if let Err(error) = place(agent, pane, self.started) {
                let message = format!("could not place {}: {error:#}", agent.name);
                diagnostics::record("projects", &message)?;
                self.failure = Some((agent.pane.clone(), message));
            }
        }
        self.started = true;
        self.failure = self
            .failure
            .take()
            .filter(|(pane, _)| panes.iter().any(|p| &p.id == pane));
        Ok(self
            .failure
            .iter()
            .map(|(_, message)| Issue::new("projects", anyhow!(message.clone())))
            .collect())
    }
}

fn place(agent: &Agent, pane: &tmux::Pane, moving: bool) -> Result<()> {
    tmux::mark_placed(&agent.pane)?;
    if !moving {
        return Ok(());
    }
    let Some(project) = project_name(&agent.cwd).filter(|name| name != &agent.session) else {
        return Ok(());
    };
    if let Some(location) = sessions::place(agent, pane.pid, &project)? {
        diagnostics::record("projects", &format!("{} joined {location}", agent.name))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_project_is_named_after_the_folder() {
        for (cwd, name) in [
            ("/Users/a/pro/api", Some("api")),
            ("/Users/a/pro/api/", Some("api")),
            ("/Users/a/two words", Some("two words")),
            ("/Users/a/my.app", Some("my-app")),
            ("/Users/a/.config", Some("config")),
            ("/Users/a/a:b", Some("a-b")),
            ("/", None),
            ("/Users/a/...", None),
        ] {
            assert_eq!(project_name(cwd).as_deref(), name, "{cwd}");
        }
    }

    #[test]
    fn every_project_name_is_a_valid_session_name() {
        for cwd in ["/a/b.c:d", "/a/ spaced ", "/a/.hidden", "/a/-dash-"] {
            sessions::validate_name(&project_name(cwd).unwrap()).unwrap();
        }
    }
}
