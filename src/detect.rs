//! Join kind-specific observations to the live tmux panes.
use crate::agents::{ProcessTable, RawState, ScanContext};
use crate::index::Kind;
use crate::tmux::Pane;
use anyhow::{Result, bail};
use clap::ValueEnum;
use std::collections::HashSet;

pub struct Detected {
    pub pane: Pane,
    pub kind: Kind,
    pub state: RawState,
    pub detail: Option<String>,
    pub title: String,
    pub session_id: Option<String>,
    pub preserve_title: bool,
    /// Durable completed turns in the published session, when available.
    pub completed_turns: Option<u64>,
}

pub fn detect(panes: &[Pane]) -> Result<Vec<Detected>> {
    let processes = ProcessTable::load()?;
    let context = ScanContext {
        panes,
        processes: &processes,
    };
    let mut found = Vec::new();
    let mut occupied = HashSet::new();
    for &kind in Kind::value_variants() {
        for observation in (kind.adapter().scan)(&context)? {
            let Some(pane) = panes.iter().find(|p| p.id == observation.pane_id) else {
                bail!(
                    "{} observed missing pane {}",
                    kind.as_str(),
                    observation.pane_id
                );
            };
            if !occupied.insert(pane.id.clone()) {
                bail!("ambiguous agent identity in pane {}", pane.id);
            }
            found.push(Detected {
                pane: pane.clone(),
                kind,
                state: observation.state,
                detail: observation.detail,
                title: observation.title,
                session_id: observation.session_id,
                preserve_title: observation.preserve_title,
                completed_turns: observation.completed_turns,
            });
        }
    }
    Ok(found)
}
