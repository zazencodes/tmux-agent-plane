//! Persistent daemon diagnostics and explicit partial batch results.
use crate::index;
use anyhow::{Context, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    pub component: String,
    pub message: String,
}

impl Issue {
    pub fn new(component: impl Into<String>, error: anyhow::Error) -> Self {
        Self {
            component: component.into(),
            message: format!("{error:#}"),
        }
    }
}

/// Successful values and scoped failures from the same operation.
#[derive(Default)]
pub struct Batch<T> {
    pub values: Vec<T>,
    pub issues: Vec<Issue>,
}

pub fn log_path() -> Result<PathBuf> {
    Ok(index::state_dir()?.join("daemon.log"))
}

pub fn record(component: &str, message: &str) -> Result<()> {
    append(&log_path()?, component, message)
}

fn append(path: &Path, component: &str, message: &str) -> Result<()> {
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("opening daemon log {}", path.display()))?;
    // Keep each record on one line, including errors originating in subprocess stderr.
    writeln!(
        log,
        "{} [{component}] {}",
        index::now_ms(),
        message.replace('\n', "\\n")
    )
    .with_context(|| format!("writing daemon log {}", path.display()))?;
    log.flush()
        .with_context(|| format!("flushing daemon log {}", path.display()))
}

#[derive(Default)]
pub struct Active {
    issues: BTreeMap<String, BTreeSet<String>>,
}

impl Active {
    /// Log changes once. Unchanged faults remain in the status on every tick.
    pub fn update(&mut self, issues: &[Issue]) -> Result<Vec<String>> {
        self.update_with(issues, record)
    }

    fn update_with(
        &mut self,
        issues: &[Issue],
        mut write: impl FnMut(&str, &str) -> Result<()>,
    ) -> Result<Vec<String>> {
        let mut current: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for issue in issues {
            current
                .entry(issue.component.clone())
                .or_default()
                .insert(issue.message.clone());
        }
        let mut messages = Vec::new();
        for (component, errors) in &current {
            for error in errors {
                if self
                    .issues
                    .get(component)
                    .is_none_or(|previous| !previous.contains(error))
                {
                    write(component, &format!("ERROR: {error}"))?;
                    messages.push(format!("{component}: {error}"));
                }
            }
        }
        for component in self.issues.keys() {
            if !current.contains_key(component) {
                write(component, "recovered")?;
                messages.push(format!("{component}: recovered"));
            }
        }
        self.issues = current;
        Ok(messages)
    }

    pub fn status(&self) -> String {
        if self.issues.is_empty() {
            String::new()
        } else {
            format!(
                "#[fg=#f38ba8]⚠ degraded: {}#[default] ",
                self.issues.keys().cloned().collect::<Vec<_>>().join(", ")
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recurring_fault_stays_visible_and_recovery_is_logged_once() {
        let mut active = Active::default();
        let issue = Issue::new(
            "scan:pi",
            anyhow::anyhow!("invalid state").context("reading fixture"),
        );
        let mut records = Vec::new();
        let mut write = |component: &str, message: &str| {
            records.push(format!("{component}: {message}"));
            Ok(())
        };
        assert_eq!(
            active
                .update_with(std::slice::from_ref(&issue), &mut write)
                .unwrap()
                .len(),
            1
        );
        assert!(active.update_with(&[issue], &mut write).unwrap().is_empty());
        assert!(active.status().contains("scan:pi"));
        assert_eq!(active.update_with(&[], &mut write).unwrap().len(), 1);
        assert!(active.update_with(&[], &mut write).unwrap().is_empty());
        assert!(active.status().is_empty());
        assert_eq!(
            records,
            [
                "scan:pi: ERROR: reading fixture: invalid state",
                "scan:pi: recovered"
            ]
        );
    }

    #[test]
    fn distinct_errors_in_same_component_are_all_recorded_without_duplicates() {
        let mut active = Active::default();
        let issues = [
            Issue::new("notification:desktop", anyhow::anyhow!("first cause")),
            Issue::new("notification:desktop", anyhow::anyhow!("second cause")),
        ];
        let mut records = Vec::new();
        let mut write = |component: &str, message: &str| {
            records.push(format!("{component}: {message}"));
            Ok(())
        };
        assert_eq!(active.update_with(&issues, &mut write).unwrap().len(), 2);
        assert!(active.update_with(&issues, &mut write).unwrap().is_empty());
        assert_eq!(records.len(), 2);
    }

    #[test]
    fn log_failure_is_fatal_and_does_not_claim_recovery() {
        let mut active = Active::default();
        let issue = Issue::new("scan:pi", anyhow::anyhow!("broken"));
        assert!(
            active
                .update_with(&[issue], |_, _| anyhow::bail!("disk full"))
                .is_err()
        );
        assert!(active.status().is_empty());
    }

    #[test]
    fn persistent_records_include_full_chain_and_survive_reopen() {
        let path =
            std::env::temp_dir().join(format!("plane-diagnostics-{}.log", std::process::id()));
        let error = anyhow::anyhow!("root cause").context("outer context");
        append(&path, "fatal", &format!("{error:#}")).unwrap();
        append(&path, "daemon", "restarted").unwrap();
        let log = std::fs::read_to_string(&path).unwrap();
        assert!(log.contains("[fatal] outer context: root cause"));
        assert!(log.contains("[daemon] restarted"));
        std::fs::remove_file(path).unwrap();
    }
}
