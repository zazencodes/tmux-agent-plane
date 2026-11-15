//! The background agent from `o` → Configure: one noninteractive command shared by naming,
//! cleanup and `background`.
use crate::autoname_config::{self as config, Config, Output};
use anyhow::{Context, Result, bail};
use std::fs::{File, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const OUTPUT_LIMIT: u64 = 16_384;
const ASK_TIMEOUT: Duration = Duration::from_secs(60);

/// The configured background command; fails when none is chosen or it is not installed.
pub fn settings() -> Result<Config> {
    let settings = config::read()?;
    if settings.auto_name.command.is_empty() {
        bail!("press o to choose a background agent first");
    }
    config::executable(&settings.auto_name.command)?;
    Ok(settings)
}

/// `background`: run the background agent once with a prompt and print its answer.
pub fn command(prompt: &str) -> Result<()> {
    let answer = ask(
        &settings()?,
        prompt,
        &crate::index::state_dir()?,
        ASK_TIMEOUT,
    )?;
    print!("{answer}");
    Ok(())
}

/// Each call writes its output files in a folder of its own, so concurrent calls never collide.
fn ask(settings: &Config, prompt: &str, state: &Path, timeout: Duration) -> Result<String> {
    let dir = state
        .join("background-work")
        .join(std::process::id().to_string());
    let answer = run_command(settings, prompt, &dir, &AtomicBool::new(false), timeout);
    let removed = std::fs::remove_dir(&dir).with_context(|| format!("removing {}", dir.display()));
    let answer = answer?;
    removed?;
    Ok(answer)
}

pub fn output_text(output: &str, format: Output) -> Result<String> {
    match format {
        Output::Text => Ok(output.to_owned()),
        Output::Opencode => {
            let mut text = String::new();
            for line in output.lines() {
                let event: serde_json::Value =
                    serde_json::from_str(line).context("parsing OpenCode output event")?;
                if event["type"] == "error" {
                    bail!("OpenCode returned an error: {event}");
                }
                if event["type"] == "text" {
                    text.push_str(
                        event["part"]["text"]
                            .as_str()
                            .context("OpenCode text event has no text")?,
                    );
                }
            }
            Ok(text)
        }
    }
}

/// Regular files cap output without a pipe deadlock; process-group kill also reaps helpers.
pub fn run_command(
    config: &Config,
    prompt: &str,
    dir: &PathBuf,
    cancel: &AtomicBool,
    timeout: Duration,
) -> Result<String> {
    let executable = config::executable(&config.auto_name.command)?;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    let stdout_path = dir.join("stdout");
    let stderr_path = dir.join("stderr");
    let file = |path: &PathBuf| -> Result<File> {
        Ok(OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(path)?)
    };
    let result = (|| {
        let args: Vec<_> = config.auto_name.command[1..]
            .iter()
            .map(|s| if s == "{prompt}" { prompt } else { s })
            .collect();
        let mut command = Command::new(executable);
        command
            .args(args)
            .current_dir(dir)
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(file(&stdout_path)?)
            .stderr(file(&stderr_path)?);
        for key in [
            "TMUX",
            "TMUX_PANE",
            "CODEX_THREAD_ID",
            "CODEX_SESSION_ID",
            "CLAUDECODE",
            "CLAUDE_CODE_SESSION_ID",
        ] {
            command.env_remove(key);
        }
        // OpenCode's plan agent can request tools; deny them in this invocation only.
        if config.auto_name.output == Output::Opencode {
            command.env("OPENCODE_CONFIG_CONTENT", r#"{"permission":{"*":"deny"}}"#);
        }
        if cancel.load(Ordering::Relaxed) {
            bail!("background cancelled");
        }
        let mut child = command.spawn().context("starting background command")?;
        let result = (|| {
            let start = Instant::now();
            loop {
                if cancel.load(Ordering::Relaxed) {
                    bail!("background cancelled");
                }
                if start.elapsed() >= timeout {
                    bail!(
                        "background command timed out after {} seconds",
                        timeout.as_secs()
                    );
                }
                if stdout_path.metadata()?.len() > OUTPUT_LIMIT
                    || stderr_path.metadata()?.len() > OUTPUT_LIMIT
                {
                    bail!("background command exceeded 16 KiB output limit");
                }
                if let Some(status) = child.try_wait()? {
                    if !status.success() {
                        bail!(
                            "background command failed ({status}): {}",
                            std::fs::read_to_string(&stderr_path)?.trim()
                        );
                    }
                    return output_text(
                        &std::fs::read_to_string(&stdout_path)?,
                        config.auto_name.output,
                    );
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        })();
        // Always end this private process group, including helpers left after parent exit.
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL);
        }
        child.wait().context("reaping background command")?;
        result
    })();
    for path in [&stdout_path, &stderr_path] {
        if path.exists() {
            std::fs::remove_file(path).context("removing background output")?;
        }
    }
    result
}

use std::os::unix::fs::DirBuilderExt;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ask_prints_the_answer_and_leaves_no_work_folder() {
        let state = std::env::temp_dir().join(format!("tap-ask-unit-{}", std::process::id()));
        let mut settings = Config::default();
        settings.auto_name.command = vec![
            "/bin/sh".into(),
            "-c".into(),
            "printf 'answer to %s' \"$0\"".into(),
            "{prompt}".into(),
        ];
        let answer = ask(&settings, "a long prompt", &state, ASK_TIMEOUT).unwrap();
        assert_eq!(answer, "answer to a long prompt");
        let work = state.join("background-work");
        assert_eq!(std::fs::read_dir(&work).unwrap().count(), 0);
        settings.auto_name.command[2] = "exit 3".into();
        assert!(ask(&settings, "fails", &state, ASK_TIMEOUT).is_err());
        assert_eq!(std::fs::read_dir(&work).unwrap().count(), 0);
        std::fs::remove_dir_all(state).unwrap();
    }
}
