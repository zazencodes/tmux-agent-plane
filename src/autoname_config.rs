//! The single XDG configuration file, shared by setup and the daemon.
use crate::index::Kind;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub auto_name: AutoName,
    pub sidebar: Sidebar,
    pub defaults: std::collections::BTreeMap<String, AgentDefaults>,
}

/// Explicit launch preferences, shared by the launcher and CLI.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct AgentDefaults {
    pub model: String,
    pub effort: String,
}

impl Config {
    pub fn spawn_args(&self, kind: Kind, explicit: Vec<String>) -> Vec<String> {
        let mut model = false;
        let mut effort = false;
        let mut flags = explicit
            .iter()
            .map(String::as_str)
            .take_while(|arg| *arg != "--");
        while let Some(arg) = flags.next() {
            let (flag, attached) = arg
                .split_once('=')
                .map_or((arg, None), |(flag, value)| (flag, Some(value)));
            let short_model =
                matches!(kind, Kind::Codex | Kind::Opencode | Kind::Grok) && arg.starts_with("-m");
            if flag == "--model" || short_model || (kind == Kind::Agy && flag == "-model") {
                model = true;
            }
            if matches!(
                flag,
                "--effort" | "--thinking" | "--variant" | "--reasoning-effort"
            ) || (kind == Kind::Agy && flag == "-effort")
            {
                effort = true;
            }
            if kind == Kind::Codex {
                let setting = if matches!(flag, "--config" | "-c") {
                    attached.or_else(|| flags.next())
                } else {
                    arg.strip_prefix("-c")
                };
                match setting
                    .and_then(|s| s.split_once('='))
                    .map(|(key, _)| key.trim())
                {
                    Some("model") => model = true,
                    Some("model_reasoning_effort") => effort = true,
                    _ => {}
                }
            }
        }
        // Effort belongs to the configured model. A one-off model chooses its own effort.
        if model {
            return explicit;
        }
        let mut defaults = self.launch_args(kind);
        if effort {
            defaults.truncate(if defaults.first().is_some_and(|arg| arg == "--model") {
                2
            } else {
                0
            });
        }
        defaults.extend(explicit);
        defaults
    }

    pub fn launch_args(&self, kind: crate::index::Kind) -> Vec<String> {
        let Some(defaults) = self.defaults.get(kind.as_str()) else {
            return Vec::new();
        };
        let mut args = Vec::new();
        if !defaults.model.is_empty() {
            args.extend(["--model".into(), defaults.model.clone()]);
        }
        if !defaults.effort.is_empty() {
            let flag = match kind {
                crate::index::Kind::Codex => "--config",
                crate::index::Kind::Pi => "--thinking",
                crate::index::Kind::Grok => "--reasoning-effort",
                crate::index::Kind::Opencode => "--variant",
                _ => "--effort",
            };
            let value = if kind == crate::index::Kind::Codex {
                format!("model_reasoning_effort=\"{}\"", defaults.effort)
            } else {
                defaults.effort.clone()
            };
            args.extend([flag.into(), value]);
        }
        args
    }
}

/// What sidebars show. `t` and `u` still show or hide one sidebar's tasks and quota.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Sidebar {
    /// Background tasks below the agents.
    pub tasks: bool,
    /// The quota panel below the graph and tasks; `u` toggles it in one sidebar.
    pub quota: bool,
    /// The selected agent's family below the list; `p` in any sidebar saves it for every sidebar.
    pub family: bool,
}

impl Default for Sidebar {
    fn default() -> Self {
        Self {
            tasks: true,
            quota: false,
            family: true,
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct AutoName {
    pub enabled: bool,
    pub preset: Option<Kind>,
    /// Executable followed by literal arguments; exactly one argument is {prompt}.
    pub command: Vec<String>,
    pub output: Output,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Output {
    #[default]
    Text,
    Opencode,
}

pub fn path() -> Result<PathBuf> {
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(base) => PathBuf::from(base),
        None => PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?).join(".config"),
    };
    Ok(base.join("tmux-agent-plane/config.json"))
}

pub fn read() -> Result<Config> {
    read_path(&path()?)
}

fn read_path(path: &Path) -> Result<Config> {
    match std::fs::read(path) {
        Ok(bytes) => parse(&bytes).with_context(|| format!("reading {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

fn parse(bytes: &[u8]) -> Result<Config> {
    let mut config: Config = serde_json::from_slice(bytes)?;
    let bg = &mut config.auto_name;
    if bg.preset.is_some() {
        anyhow::ensure!(
            bg.command.is_empty(),
            "choose a background preset or a custom command"
        );
        return Ok(config);
    }
    if let Some(kind) = legacy_preset(&bg.command, bg.output)
        && serde_json::from_slice::<serde_json::Value>(bytes)?["auto_name"]
            .get("preset")
            .is_none()
    {
        bg.preset = Some(kind);
        bg.command.clear();
        return Ok(config);
    }
    if config.auto_name.enabled || !config.auto_name.command.is_empty() {
        validate_command(&config.auto_name.command)?;
    }
    Ok(config)
}

pub fn validate_command(command: &[String]) -> Result<()> {
    if command
        .first()
        .is_none_or(|s| s.is_empty() || s.contains('{'))
        || command.iter().filter(|s| *s == "{prompt}").count() != 1
        || command
            .iter()
            .any(|s| s.contains('\0') || (s.contains("{prompt}") && s != "{prompt}"))
    {
        bail!("command needs an executable and one literal {{prompt}} argument");
    }
    Ok(())
}

/// Check presence without executing a CLI, authenticating, or spending tokens.
pub fn executable(command: &[String]) -> Result<PathBuf> {
    validate_command(command)?;
    let name = &command[0];
    let candidates = if name.contains('/') {
        let path = PathBuf::from(name);
        if !path.is_absolute() {
            bail!("use an absolute executable path or a command on PATH");
        }
        vec![path]
    } else {
        std::env::split_paths(&std::env::var_os("PATH").context("PATH is not set")?)
            .map(|dir| dir.join(name))
            .collect()
    };
    candidates
        .into_iter()
        .find(|path| {
            path.metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
        .with_context(|| format!("command {name:?} is not installed or executable"))
}

pub fn write(config: &Config) -> Result<()> {
    let path = path()?;
    // Validate the same bytes that readers will see before replacing the file.
    let bytes = serde_json::to_vec_pretty(config)?;
    parse(&bytes)?;
    std::fs::create_dir_all(path.parent().unwrap())?;
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .context("creating configuration temporary file")?;
    use std::io::Write;
    let result = (|| {
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::rename(&tmp, &path).context("saving auto-name configuration")
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(tmp);
    }
    result
}

pub fn preset_kind(choice: char) -> Option<Kind> {
    Some(match choice {
        '1' => Kind::Claude,
        '2' => Kind::Codex,
        '3' => Kind::Pi,
        '4' => Kind::Opencode,
        '5' => Kind::Agy,
        '6' => Kind::Grok,
        _ => return None,
    })
}

impl Config {
    pub fn preset_command(&self, kind: Kind) -> Vec<String> {
        let tail: &[&str] = match kind {
            Kind::Claude => &[
                "--print",
                "--output-format",
                "text",
                "--tools",
                "",
                "--strict-mcp-config",
                "--no-session-persistence",
                "{prompt}",
            ],
            Kind::Codex => &[
                "--ephemeral",
                "--skip-git-repo-check",
                "--sandbox",
                "read-only",
                "--color",
                "never",
                "{prompt}",
            ],
            Kind::Pi => &[
                "--print",
                "--no-session",
                "--no-tools",
                "--no-extensions",
                "--no-skills",
                "--no-context-files",
                "{prompt}",
            ],
            Kind::Opencode => &["--pure", "--format", "json", "--agent", "plan", "{prompt}"],
            Kind::Agy => &[
                "--output-format",
                "text",
                "--mode",
                "plan",
                "--sandbox",
                "--print-timeout",
                "55s",
                "--print",
                "{prompt}",
            ],
            Kind::Grok => &[
                "--tools",
                "",
                "--disable-web-search",
                "--no-subagents",
                "--max-turns",
                "1",
                "--single",
                "{prompt}",
            ],
        };
        let mut command = vec![kind.as_str().into()];
        match kind {
            Kind::Codex => command.push("exec".into()),
            Kind::Opencode => command.push("run".into()),
            _ => {}
        }
        command.extend(self.launch_args(kind));
        command.extend(tail.iter().map(|s| s.to_string()));
        command
    }

    pub fn resolve_background(mut self) -> Self {
        if let Some(kind) = self.auto_name.preset {
            self.auto_name.command = self.preset_command(kind);
        }
        self
    }
}
// Only exact commands saved by previous releases are migrated.
fn legacy_preset(command: &[String], output: Output) -> Option<Kind> {
    let (kind, old_flags): (Kind, &[&str]) = match command.first()?.as_str() {
        "claude" => (Kind::Claude, &["--model", "haiku", "--effort", "low"]),
        "codex" => (
            Kind::Codex,
            &[
                "--model",
                "gpt-6-luna",
                "--config",
                "model_reasoning_effort=\"low\"",
            ],
        ),
        "pi" => (
            Kind::Pi,
            &[
                "--provider",
                "google",
                "--model",
                "gemini-flash-lite-latest",
                "--thinking",
                "minimal",
            ],
        ),
        "opencode" => (Kind::Opencode, &["--model", "opencode/ling-2.6-flash-free"]),
        "agy" => (
            Kind::Agy,
            &["--model", "gemini-3.8-flash", "--effort", "low"],
        ),
        "grok" => (
            Kind::Grok,
            &["--model", "grok-4.5", "--reasoning-effort", "low"],
        ),
        _ => return None,
    };
    if (kind == Kind::Opencode) != (output == Output::Opencode) {
        return None;
    }
    let mut old = Config::default().preset_command(kind);
    let pos = if matches!(kind, Kind::Codex | Kind::Opencode) {
        2
    } else {
        1
    };
    old.splice(pos..pos, old_flags.iter().map(|s| s.to_string()));
    (old == command).then_some(kind)
}
/// Describe exactly the explicit settings in the reviewed command array.
pub fn model_summary(command: &[String]) -> String {
    let value = |flag: &str| {
        command
            .windows(2)
            .find(|pair| pair[0] == flag)
            .map(|pair| pair[1].as_str())
    };
    let model = value("--model");
    let effort = value("--effort")
        .or_else(|| value("--thinking"))
        .or_else(|| value("--reasoning-effort"))
        .or_else(|| {
            command.iter().find_map(|arg| {
                arg.strip_prefix("model_reasoning_effort=\"")?
                    .strip_suffix('"')
            })
        });
    format!(
        "Model: {}\nThinking: {}\n",
        model.unwrap_or("agent default"),
        effort.unwrap_or("automatic")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_launch_defaults_round_trip() {
        let cfg =
            parse(br#"{"defaults":{"codex":{"model":"test-model","effort":"medium"}}}"#).unwrap();
        assert_eq!(
            cfg.launch_args(crate::index::Kind::Codex),
            [
                "--model",
                "test-model",
                "--config",
                "model_reasoning_effort=\"medium\""
            ]
        );
        assert!(cfg.launch_args(crate::index::Kind::Claude).is_empty());
        assert_eq!(parse(&serde_json::to_vec(&cfg).unwrap()).unwrap(), cfg);
    }

    #[test]
    fn unrelated_spawn_flags_keep_defaults_and_overrides_win() {
        let cfg = parse(br#"{"defaults":{"codex":{"model":"saved","effort":"high"}}}"#).unwrap();
        let args = |items: &[&str]| {
            cfg.spawn_args(Kind::Codex, items.iter().map(|s| s.to_string()).collect())
        };
        assert_eq!(
            args(&["--no-alt-screen"]),
            [
                "--model",
                "saved",
                "--config",
                "model_reasoning_effort=\"high\"",
                "--no-alt-screen"
            ]
        );
        assert_eq!(args(&["-m", "other"]), ["-m", "other"]);
        assert_eq!(
            args(&["-c", "model_reasoning_effort='low'"]),
            ["--model", "saved", "-c", "model_reasoning_effort='low'"]
        );
    }

    #[test]
    fn agy_takes_the_prompt_as_the_value_of_print() {
        let command = Config::default().preset_command(Kind::Agy);
        let print = command.iter().position(|arg| arg == "--print").unwrap();
        assert_eq!(command[print + 1], "{prompt}");
    }

    #[test]
    fn saved_background_preset_follows_the_current_launch_defaults() {
        let cfg = parse(br#"{"auto_name":{"command":["agy","--model","gemini-3.8-flash","--effort","low","--output-format","text","--mode","plan","--sandbox","--print-timeout","55s","--print","{prompt}"]},"defaults":{"agy":{"model":"new-model","effort":"high"}}}"#).unwrap();
        assert_eq!(cfg.auto_name.preset, Some(Kind::Agy));
        assert!(cfg.auto_name.command.is_empty());
        assert_eq!(
            cfg.clone().resolve_background().auto_name.command[1..5],
            cfg.launch_args(Kind::Agy)
        );
        assert_eq!(parse(&serde_json::to_vec(&cfg).unwrap()).unwrap(), cfg);
    }

    #[test]
    fn absent_and_empty_config_are_off() {
        let config = read_path(Path::new("/absent-auto-name-fixture/config.json")).unwrap();
        assert_eq!(config, parse(b"{}").unwrap());
        assert!(!config.auto_name.enabled);
        assert!(config.auto_name.command.is_empty());
    }
    #[test]
    fn sidebars_show_tasks_and_hide_the_quota_panel_unless_configured() {
        let config = parse(b"{}").unwrap();
        assert!(config.sidebar.tasks && !config.sidebar.quota && config.sidebar.family);
        let config = parse(br#"{"sidebar":{"family":false}}"#).unwrap();
        assert!(!config.sidebar.family && config.sidebar.tasks);
        let config = parse(br#"{"sidebar":{"quota":true}}"#).unwrap();
        assert!(config.sidebar.tasks && config.sidebar.quota);
        let config = parse(br#"{"sidebar":{"tasks":false}}"#).unwrap();
        assert!(!config.sidebar.tasks && !config.sidebar.quota);
        assert!(parse(br#"{"sidebar":{"usage":true}}"#).is_err());
    }
    #[test]
    fn removed_settings_fail_loudly() {
        assert!(parse(br#"{"auto_name":{"hide_hint":true}}"#).is_err());
        assert!(parse(br#"{"reviews":{"enabled":true}}"#).is_err());
    }
    #[test]
    fn invalid_configuration_fails_loudly() {
        for bytes in [
            b"invalid".as_slice(),
            br#"{"auto_name":{"enabled":true}}"#,
            br#"{"auto_name":{"enabeld":true}}"#,
            br#"{"unknown":true}"#,
            br#"{"auto_name":{"command":["foo","prefix {prompt}"]}}"#,
            br#"{"auto_name":{"output":"json"}}"#,
            br#"{"reviews":{"interval_days":0}}"#,
            br#"{"reviews":{"interval_days":"90"}}"#,
            br#"{"reviews":{"enabled":null}}"#,
            br#"{"reviews":{"last_reviewed":{"auto_name":"yesterday"}}}"#,
            br#"{"reviews":{"last_reviewed":{"auto_name":null}}}"#,
            br#"{"reviews":{"last_reviewed":{"auto_name":-1}}}"#,
            br#"{"reviews":{"last_reviewed":{"auto_name":1.5}}}"#,
            br#"{"reviews":{"last_reviewed":{"auto_name":18446744073709551615}}}"#,
        ] {
            assert!(parse(bytes).is_err());
        }
        for choice in '1'..='6' {
            validate_command(&Config::default().preset_command(preset_kind(choice).unwrap()))
                .unwrap();
        }
    }
}
