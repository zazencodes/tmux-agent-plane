//! The single XDG configuration file, shared by setup and the daemon.
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub auto_name: AutoName,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct AutoName {
    pub enabled: bool,
    pub hide_hint: bool,
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
            "--print",
            "--no-session",
            "--no-tools",
            "--no-extensions",
            "--no-skills",
            "--no-context-files",
            "{prompt}",
        ],
        '4' => &[
            "opencode", "run", "--pure", "--format", "json", "--agent", "plan", "{prompt}",
        ],
        '5' => &[
            "agy",
            "--print",
            "--output-format",
            "text",
            "--disable-slash-commands",
            "--mode",
            "plan",
            "--sandbox",
            "--print-timeout",
            "55s",
            "{prompt}",
        ],
        '6' => &[
            "grok",
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn absent_and_empty_config_are_off_and_hint_is_visible() {
        let config = read_path(Path::new("/absent-auto-name-fixture/config.json")).unwrap();
        assert_eq!(config, parse(b"{}").unwrap());
        assert!(!config.auto_name.enabled);
        assert!(!config.auto_name.hide_hint);
        assert!(config.auto_name.command.is_empty());
    }
    #[test]
    fn hide_flag_round_trips_without_enabling() {
        let config = parse(br#"{"auto_name":{"hide_hint":true}}"#).unwrap();
        assert!(config.auto_name.hide_hint);
        assert!(!config.auto_name.enabled);
        assert_eq!(
            parse(&serde_json::to_vec(&config).unwrap()).unwrap(),
            config
        );
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
        ] {
            assert!(parse(bytes).is_err());
        }
        for choice in '1'..='6' {
            validate_command(&preset(choice).unwrap()).unwrap();
        }
    }
}
