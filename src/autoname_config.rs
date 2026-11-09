//! The single XDG configuration file, shared by setup and the daemon.
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub auto_name: AutoName,
    pub sidebar: Sidebar,
}

/// What a sidebar shows when it opens. `t` still shows or hides one sidebar's tasks.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Sidebar {
    /// Background tasks below the agents.
    pub tasks: bool,
    /// The quota panel below the list; `u` opens the full view either way.
    pub quota: bool,
    /// The selected agent's family below the list; `p` still shows or hides it in one sidebar.
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
    let config: Config = serde_json::from_slice(bytes)?;
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

pub fn preset(choice: char) -> Option<Vec<String>> {
    let args: &[&str] = match choice {
        '1' => &[
            "claude",
            "--model",
            "haiku",
            "--effort",
            "low",
            "--print",
            "--output-format",
            "text",
            "--tools",
            "",
            "--strict-mcp-config",
            "--no-session-persistence",
            "{prompt}",
        ],
        '2' => &[
            "codex",
            "exec",
            "--model",
            "gpt-6-luna",
            "--config",
            "model_reasoning_effort=\"low\"",
            "--ephemeral",
            "--skip-git-repo-check",
            "--sandbox",
            "read-only",
            "--color",
            "never",
            "{prompt}",
        ],
        '3' => &[
            "pi",
            "--provider",
            "google",
            "--model",
            "gemini-flash-lite-latest",
            "--thinking",
            "minimal",
            "--print",
            "--no-session",
            "--no-tools",
            "--no-extensions",
            "--no-skills",
            "--no-context-files",
            "{prompt}",
        ],
        '4' => &[
            "opencode",
            "run",
            "--model",
            "opencode/ling-2.6-flash-free",
            "--pure",
            "--format",
            "json",
            "--agent",
            "plan",
            "{prompt}",
        ],
        '5' => &[
            "agy",
            "--model",
            "gemini-3.8-flash",
            "--effort",
            "low",
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
        '6' => &[
            "grok",
            "--model",
            "grok-4.5",
            "--reasoning-effort",
            "low",
            "--tools",
            "",
            "--disable-web-search",
            "--no-subagents",
            "--max-turns",
            "1",
            "--single",
            "{prompt}",
        ],
        _ => return None,
    };
    Some(args.iter().map(|s| s.to_string()).collect())
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
    match (model, effort) {
        (Some(model), Some(effort)) => format!("Model: {model}\nThinking: {effort}\n"),
        (Some("opencode/ling-2.6-flash-free"), None) => {
            "Model: Ling 2.6 Flash Free\nThinking: off (non-reasoning model)\n".into()
        }
        _ => "Model / thinking: custom command\n".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn agy_takes_the_prompt_as_the_value_of_print() {
        let command = preset('5').unwrap();
        let print = command.iter().position(|arg| arg == "--print").unwrap();
        assert_eq!(command[print + 1], "{prompt}");
    }

    #[test]
    fn presets_pin_the_exact_model_and_lowest_verified_thinking_argv() {
        let expected: [&[&str]; 6] = [
            &[
                "claude",
                "--model",
                "haiku",
                "--effort",
                "low",
                "--print",
                "--output-format",
                "text",
                "--tools",
                "",
                "--strict-mcp-config",
                "--no-session-persistence",
                "{prompt}",
            ],
            &[
                "codex",
                "exec",
                "--model",
                "gpt-6-luna",
                "--config",
                "model_reasoning_effort=\"low\"",
                "--ephemeral",
                "--skip-git-repo-check",
                "--sandbox",
                "read-only",
                "--color",
                "never",
                "{prompt}",
            ],
            &[
                "pi",
                "--provider",
                "google",
                "--model",
                "gemini-flash-lite-latest",
                "--thinking",
                "minimal",
                "--print",
                "--no-session",
                "--no-tools",
                "--no-extensions",
                "--no-skills",
                "--no-context-files",
                "{prompt}",
            ],
            &[
                "opencode",
                "run",
                "--model",
                "opencode/ling-2.6-flash-free",
                "--pure",
                "--format",
                "json",
                "--agent",
                "plan",
                "{prompt}",
            ],
            &[
                "agy",
                "--model",
                "gemini-3.8-flash",
                "--effort",
                "low",
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
            &[
                "grok",
                "--model",
                "grok-4.5",
                "--reasoning-effort",
                "low",
                "--tools",
                "",
                "--disable-web-search",
                "--no-subagents",
                "--max-turns",
                "1",
                "--single",
                "{prompt}",
            ],
        ];
        for (choice, expected) in ('1'..='6').zip(expected) {
            let command = preset(choice).unwrap();
            assert_eq!(command, expected);
            let summary = model_summary(&command);
            assert!(summary.contains("Model:"));
            assert!(summary.contains("Thinking:"));
            assert!(!summary.contains("custom command"));
        }
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
            validate_command(&preset(choice).unwrap()).unwrap();
        }
    }
}
