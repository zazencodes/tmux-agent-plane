//! Own the plugin's bindings only while enabled, and restore what each one replaced.
use crate::{sidebar, tmux};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

const SAVED: &str = "@agent-plane-bindings";
const KEYS: &str = "@agent-plane-keys";

#[derive(Serialize, Deserialize)]
struct Binding {
    table: String,
    key: String,
    original: Option<String>,
    installed: String,
}

struct Key {
    table: &'static str,
    key: String,
    command: Vec<String>,
}

impl Key {
    fn new(table: &'static str, key: &str, command: &[&str]) -> Self {
        Self {
            table,
            key: key.into(),
            command: command.iter().map(|s| (*s).into()).collect(),
        }
    }
}

fn option(name: &str) -> Result<String> {
    Ok(tmux::run(&["show-option", "-gqv", name])?.trim().into())
}

fn configured(name: &str, default: &str) -> Result<String> {
    let value = option(&format!("@agent-plane-{name}-key"))?;
    Ok(if value.is_empty() {
        default.into()
    } else {
        value
    })
}

pub(crate) fn disabled() -> Result<bool> {
    match option(KEYS)?.as_str() {
        "off" => Ok(true),
        "" | "on" => Ok(false),
        _ => bail!("@agent-plane-keys must be on or off"),
    }
}

pub(crate) fn require_enabled() -> Result<()> {
    if disabled()? {
        let key = configured("keys-toggle", "o")?;
        bail!("Agent Plane keys are off; press prefix + {key} to enable them");
    }
    Ok(())
}

/// Parse list-keys without rewriting its quoting: each original line is valid tmux source.
fn rows(output: &str) -> Vec<(&str, &str, &str)> {
    output
        .lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            words.find(|word| *word == "-T")?;
            Some((words.next()?, words.next()?, line))
        })
        .collect()
}

fn find<'a>(output: &'a str, table: &str, key: &str) -> Option<&'a str> {
    rows(output)
        .into_iter()
        .find_map(|(t, k, line)| (t == table && k == key).then_some(line))
}

fn same(a: &str, b: &str) -> bool {
    // Only the binding header is padded. Whitespace inside a quoted command is meaningful.
    fn parts(line: &str) -> Option<(&str, &str)> {
        let (flags, mut rest) = line.split_once(" -T ")?;
        for _ in 0..2 {
            rest = rest.trim_start();
            rest = &rest[rest.find(char::is_whitespace)?..];
        }
        Some((flags, rest.trim_start()))
    }
    match (parts(a), parts(b)) {
        (Some((a_flags, a_command)), Some((b_flags, b_command))) => {
            a_flags.split_whitespace().eq(b_flags.split_whitespace()) && a_command == b_command
        }
        _ => false,
    }
}

fn restore(table: &str, key: &str, original: Option<&str>) -> Result<()> {
    tmux::run(&["unbind-key", "-T", table, key])?;
    if let Some(line) = original {
        tmux::run_with_stdin(&["source-file", "-"], &format!("{line}\n"))?;
    }
    Ok(())
}

/// The pre-snapshot plugin already replaced these native bindings. On its first upgrade,
/// return those keys to tmux's defaults; subsequent installs preserve exact user bindings.
fn remove_legacy(output: &str) -> Result<()> {
    let toggle = option("@agent-plane-bound-keys-toggle-key")?;
    if toggle.is_empty() {
        return Ok(());
    }
    let sidebar = configured("sidebar", "S")?;
    let mut owned = HashSet::new();
    for name in ["previous", "next", "sidebar-in", "sidebar-out"] {
        for key in option(&format!("@agent-plane-bound-{name}-key"))?.split_whitespace() {
            owned.insert(("root", key.to_string()));
        }
    }
    for key in ["a", "f", &sidebar, &toggle] {
        owned.insert(("prefix", key.to_string()));
    }
    for (table, key, line) in rows(output) {
        if !owned.contains(&(table, key.to_string())) {
            continue;
        }
        if !line.contains("tmux-agent-plane") && !line.contains("@agent-plane-keys") {
            continue;
        }
        let original = match (table, key) {
            ("prefix", "Space") => Some("bind-key -T prefix Space next-layout"),
            ("prefix", "o") => Some("bind-key -T prefix o select-pane -t :.+"),
            ("prefix", "f") => {
                Some("bind-key -T prefix f command-prompt { find-window -Z \"%%\" }")
            }
            _ => None,
        };
        restore(table, key, original)?;
    }
    Ok(())
}

pub fn apply(toggle: bool) -> Result<()> {
    let _lock = sidebar::Lock::take()?;
    let off = disabled()? ^ toggle;
    let mut keys = Vec::new();
    let mut bound = Vec::new();
    for (name, default) in [
        ("previous", "M-k"),
        ("next", "M-j"),
        ("sidebar-in", "M-h"),
        ("sidebar-out", "M-l"),
    ] {
        let value = configured(name, default)?;
        for key in value.split_whitespace() {
            let command = match name {
                "previous" | "next" => vec![
                    "if-shell".into(),
                    "-F".into(),
                    "#{==:#{@agent-plane-sidebar},1}".into(),
                    format!(
                        "send-keys {}",
                        if name == "previous" { "Up" } else { "Down" }
                    ),
                    format!(
                        "run-shell 'tmux-agent-plane cycle {name} --client #{{q:client_name}}'"
                    ),
                ],
                "sidebar-in" => vec![
                    "run-shell".into(),
                    "tmux-agent-plane enter-sidebar '#{window_id}'".into(),
                ],
                "sidebar-out" => vec![
                    "if-shell".into(),
                    "-F".into(),
                    "#{==:#{@agent-plane-sidebar},1}".into(),
                    "send-keys S-Right".into(),
                    format!("send-keys {key}"),
                ],
                _ => unreachable!(),
            };
            keys.push(Key {
                table: "root",
                key: key.into(),
                command,
            });
        }
        bound.push((name, value));
    }
    let sidebar = configured("sidebar", "S")?;
    keys.push(Key::new(
        "prefix",
        &sidebar,
        &[
            "run-shell",
            "tmux-agent-plane toggle-sidebar '#{window_id}'",
        ],
    ));
    for (key, search) in [("a", ""), ("f", " --search")] {
        keys.push(Key::new("prefix", key, &["run-shell", "-C", &format!(
            "display-popup -E -B -w 100% -h 100% -s 'bg=terminal,fg=terminal' \"tmux-agent-plane ui --client #{{q:client_name}}{search}\""
        )]));
    }
    let toggle_key = configured("keys-toggle", "o")?;
    keys.push(Key::new(
        "prefix",
        &toggle_key,
        &["run-shell", "tmux-agent-plane bindings --toggle"],
    ));
    let mut seen = HashSet::new();
    for key in &keys {
        if !seen.insert((key.table, &key.key)) {
            bail!(
                "each Agent Plane key needs one action; repeated: {} {}",
                key.table,
                key.key
            );
        }
    }
    // Validation precedes any changes. Share the sidebar lock so toggling keys cannot race
    // an explicit sidebar open or close.
    let saved = option(SAVED)?;
    let current = tmux::run(&["list-keys"])?;
    if saved.is_empty() {
        remove_legacy(&current)?;
    } else {
        let bindings: Vec<Binding> =
            serde_json::from_str(&saved).context("invalid saved Agent Plane bindings")?;
        for binding in bindings {
            if find(&current, &binding.table, &binding.key)
                .is_some_and(|line| same(line, &binding.installed))
            {
                restore(&binding.table, &binding.key, binding.original.as_deref())?;
            }
        }
    }
    if off {
        tmux::run(&["set-option", "-g", KEYS, "off"])?;
    }
    let originals = tmux::run(&["list-keys"])?;
    let mut bindings = Vec::new();
    for key in keys
        .iter()
        .filter(|key| !off || (key.table == "prefix" && key.key == toggle_key))
    {
        let mut args = vec!["bind-key", "-T", key.table, &key.key];
        args.extend(key.command.iter().map(String::as_str));
        tmux::run(&args)?;
        bindings.push(Binding {
            table: key.table.into(),
            key: key.key.clone(),
            original: find(&originals, key.table, &key.key).map(str::to_owned),
            installed: String::new(),
        });
    }
    let installed = tmux::run(&["list-keys"])?;
    for binding in &mut bindings {
        binding.installed = find(&installed, &binding.table, &binding.key)
            .context("installed Agent Plane key is missing")?
            .into();
    }
    tmux::run(&[
        "set-option",
        "-g",
        SAVED,
        &serde_json::to_string(&bindings)?,
    ])?;
    bound.push(("keys-toggle", toggle_key));
    for (name, value) in bound {
        tmux::run(&[
            "set-option",
            "-g",
            &format!("@agent-plane-bound-{name}-key"),
            &value,
        ])?;
    }
    if !off {
        tmux::run(&["set-option", "-g", KEYS, "on"])?;
    }
    if toggle {
        crate::lifecycle::run(crate::lifecycle::Action::Tick)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ownership_ignores_column_padding_but_preserves_command_text_and_repeat() {
        let installed = "bind-key    -T prefix a    run-shell \"echo two  spaces\"";
        assert!(same(
            installed,
            "bind-key -T prefix     a run-shell \"echo two  spaces\""
        ));
        assert!(!same(
            installed,
            "bind-key -T prefix a run-shell \"echo two spaces\""
        ));
        assert!(!same(
            installed,
            "bind-key -r -T prefix a run-shell \"echo two  spaces\""
        ));
    }
}
