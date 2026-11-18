use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use serde_json::Value;
use std::{
    io::Write,
    process::{Command, Stdio},
    time::Duration,
};

#[derive(Default, Deserialize)]
pub struct Model {
    #[serde(alias = "value")]
    pub id: String,
    #[serde(default, rename = "supportedEffortLevels")]
    pub efforts: Vec<String>,
    #[serde(default, rename = "displayName")]
    pub name: String,
}

pub fn load(kind: &str) -> Result<Vec<Model>> {
    let models = match kind {
        "codex" => crate::quota::codex::models()?
            .into_iter()
            .map(|v| {
                let mut model = Model {
                    id: v["model"]
                        .as_str()
                        .context("missing Codex model ID")?
                        .into(),
                    name: v["displayName"]
                        .as_str()
                        .context("missing Codex model name")?
                        .into(),
                    efforts: Vec::new(),
                };
                for level in v["supportedReasoningEfforts"]
                    .as_array()
                    .context("missing Codex effort levels")?
                {
                    model.efforts.push(
                        level["reasoningEffort"]
                            .as_str()
                            .context("invalid Codex effort")?
                            .into(),
                    );
                }
                Ok(model)
            })
            .collect::<Result<Vec<_>>>()?,
        "claude" => {
            let initialize = serde_json::json!({"type":"control_request","request_id":"catalog","request":{"subtype":"initialize"}});
            let output = run(
                "claude",
                &[
                    "-p",
                    "--input-format",
                    "stream-json",
                    "--output-format",
                    "stream-json",
                    "--verbose",
                    "--no-session-persistence",
                ],
                Some(format!("{initialize}\n").as_bytes()),
            )?;
            let mut catalog = None;
            for line in output.lines() {
                let msg: Value = serde_json::from_str(line)?;
                if msg.pointer("/response/request_id").and_then(Value::as_str) == Some("catalog") {
                    catalog = Some(serde_json::from_value::<Vec<Model>>(
                        msg.pointer("/response/response/models")
                            .context("Claude returned no models")?
                            .clone(),
                    )?);
                    break;
                }
            }
            catalog
                .context("Claude did not return a model catalog")?
                .into_iter()
                .filter(|m| m.id != "default")
                .collect()
        }
        "agy" => agy_models(&run("agy", &["models"], None)?)?,
        _ => bail!("No catalog for {kind}"),
    };
    ensure!(!models.is_empty(), "{kind} returned no models");
    Ok(models)
}

fn agy_models(output: &str) -> Result<Vec<Model>> {
    let mut models: Vec<Model> = Vec::new();
    for line in output.lines().filter(|line| !line.trim().is_empty()) {
        let (slug, label) = line
            .trim()
            .split_once(char::is_whitespace)
            .context("unexpected agy models output")?;
        let variant = slug
            .rsplit_once('-')
            .filter(|(_, level)| matches!(*level, "low" | "medium" | "high" | "xhigh" | "max"));
        let id = variant.map_or(slug, |(id, _)| id);
        let idx = models.iter().position(|m| m.id == id).unwrap_or_else(|| {
            models.push(Model {
                id: id.into(),
                name: label.trim().to_string(),
                ..Default::default()
            });
            models.len() - 1
        });
        if let Some((_, level)) = variant {
            models[idx].efforts.push(level.into());
            if let Some((base, _)) = label.trim().rsplit_once(" (") {
                models[idx].name = base.into();
            }
        }
    }
    for model in &mut models {
        model.efforts.sort_by_key(|level| {
            ["low", "medium", "high", "xhigh", "max"]
                .iter()
                .position(|&name| name == level)
        });
    }
    Ok(models)
}

fn run(bin: &str, args: &[&str], input: Option<&[u8]>) -> Result<String> {
    use crate::quota::fetch::{group, wait_output};
    let mut child = group(&mut Command::new(bin))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("starting {bin}"))?;
    let mut stdin = child.stdin.take().unwrap();
    if let Some(data) = input
        && let Err(error) = stdin.write_all(data)
    {
        crate::quota::fetch::stop(&mut child);
        return Err(error.into());
    }
    drop(stdin);
    let (status, bytes) =
        wait_output(child, Duration::from_secs(15), 4 << 20).map_err(anyhow::Error::msg)?;
    ensure!(
        status == Some(0),
        "{bin} model lookup failed. Check your CLI sign-in."
    );
    Ok(String::from_utf8(bytes)?)
}

#[test]
fn groups_agy_variants_and_preserves_fixed_models() {
    let models =
        agy_models("flash-high  Flash (High)\norchestrator  Orchestrator\nflash-low  Flash (Low)")
            .unwrap();
    assert_eq!(models[0].id, "flash");
    assert_eq!(models[0].efforts, ["low", "high"]);
    assert!(models[1].efforts.is_empty());
}

#[test]
#[ignore = "requires signed-in agent CLIs"]
fn installed_catalogs() {
    for kind in ["codex", "claude", "agy"] {
        let models = load(kind).unwrap();
        assert!(
            models
                .iter()
                .any(|m| !m.id.is_empty() && !m.efforts.is_empty()),
            "{kind}"
        );
        println!("{kind}: {} models", models.len());
    }
}
