//! Join kind-specific observations to the live tmux panes.
use crate::agents::{Observation, ProcessTable, RawState, ScanContext};
use crate::diagnostics::{Batch, Issue};
use crate::index::Kind;
use crate::tmux::Pane;
use anyhow::{Context, Result, anyhow};
use clap::ValueEnum;
use std::collections::HashMap;

pub struct Detected {
    pub pane: Pane,
    pub kind: Kind,
    pub state: RawState,
    pub detail: Option<String>,
    pub title: String,
    pub session_id: Option<String>,
    pub preserve_title: bool,
    /// Monotonic completed turns within completion_epoch, when available.
    pub completed_turns: Option<u64>,
    /// Stable publisher lifetime for the monotonic completion counter.
    pub completion_epoch: Option<String>,
}

pub fn detect(panes: &[Pane]) -> Result<Batch<Detected>> {
    // A shared process-table failure invalidates the whole scan and is fatal.
    let processes = ProcessTable::load().context("loading process table")?;
    let context = ScanContext {
        panes,
        processes: &processes,
    };
    Ok(scan_kinds(panes, |kind| (kind.adapter().scan)(&context)))
}

fn scan_kinds(
    panes: &[Pane],
    mut scan: impl FnMut(Kind) -> Result<Vec<Observation>>,
) -> Batch<Detected> {
    let mut batch = Batch {
        values: Vec::new(),
        issues: Vec::new(),
    };
    let mut occupied: HashMap<String, Vec<Kind>> = HashMap::new();
    let mut invalid = Vec::new();
    for &kind in Kind::value_variants() {
        let observations = match scan(kind) {
            Ok(observations) => observations,
            Err(error) => {
                batch
                    .issues
                    .push(Issue::new(format!("scan:{}", kind.as_str()), error));
                continue;
            }
        };
        for observation in observations {
            let Some(pane) = panes.iter().find(|p| p.id == observation.pane_id) else {
                invalid.push(kind);
                batch.issues.push(Issue::new(
                    format!("scan:{}", kind.as_str()),
                    anyhow!("observed missing pane {}", observation.pane_id),
                ));
                continue;
            };
            occupied.entry(pane.id.clone()).or_default().push(kind);
            batch.values.push(Detected {
                pane: pane.clone(),
                kind,
                state: observation.state,
                detail: observation.detail,
                title: observation.title,
                session_id: observation.session_id,
                preserve_title: observation.preserve_title,
                completed_turns: observation.completed_turns,
                completion_epoch: observation.completion_epoch,
            });
        }
    }
    // Neither claimant can be trusted. Do not give priority to adapter order.
    for (pane, kinds) in occupied {
        if kinds.len() > 1 {
            for kind in kinds {
                invalid.push(kind);
                batch.issues.push(Issue::new(
                    format!("scan:{}", kind.as_str()),
                    anyhow!("ambiguous agent identity in pane {pane}"),
                ));
            }
        }
    }
    batch.values.retain(|d| !invalid.contains(&d.kind));
    batch
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(id: &str) -> Pane {
        Pane {
            id: id.into(),
            pid: 1,
            session: "test".into(),
            window: "@1".into(),
            window_index: 1,
            pane_index: 0,
            cwd: "/tmp".into(),
            title: "task".into(),
            active: false,
            viewed: false,
            sidebar: false,
            name: String::new(),
            parent: String::new(),
        }
    }
    fn observation(id: &str) -> Observation {
        Observation {
            pane_id: id.into(),
            state: RawState::Working,
            detail: None,
            title: "task".into(),
            session_id: None,
            preserve_title: false,
            completed_turns: None,
            completion_epoch: None,
        }
    }

    #[test]
    fn kind_fault_does_not_discard_other_kinds_and_next_scan_recovers() {
        let panes = [pane("%1"), pane("%2")];
        for failed in [true, false] {
            let batch = scan_kinds(&panes, |kind| match kind {
                Kind::Claude if failed => Err(anyhow!("unknown state").context("fixture session")),
                Kind::Claude => Ok(vec![observation("%1")]),
                Kind::Codex => Ok(vec![observation("%2")]),
                _ => Ok(Vec::new()),
            });
            assert!(batch.values.iter().any(|d| d.kind == Kind::Codex));
            assert_eq!(batch.values.len(), if failed { 1 } else { 2 });
            assert_eq!(batch.issues.len(), usize::from(failed));
            if failed {
                assert_eq!(batch.issues[0].component, "scan:claude");
                assert_eq!(batch.issues[0].message, "fixture session: unknown state");
            }
        }
    }

    #[test]
    fn ambiguous_claims_withdraw_both_kinds_and_missing_pane_withdraws_kind() {
        let panes = [pane("%1"), pane("%2")];
        let batch = scan_kinds(&panes, |kind| match kind {
            Kind::Claude | Kind::Codex => Ok(vec![observation("%1")]),
            Kind::Pi => Ok(vec![observation("%2")]),
            _ => Ok(Vec::new()),
        });
        assert_eq!(batch.values.len(), 1);
        assert_eq!(batch.values[0].kind, Kind::Pi);
        assert_eq!(batch.issues.len(), 2);
        let batch = scan_kinds(&panes, |kind| match kind {
            Kind::Claude => Ok(vec![observation("%1"), observation("%missing")]),
            _ => Ok(Vec::new()),
        });
        assert!(batch.values.is_empty());
        assert_eq!(batch.issues.len(), 1);
    }
}
