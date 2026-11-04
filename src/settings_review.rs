//! A static list of settings worth reviewing. This module never runs a command.
use crate::autoname_config::{self as config, Config};
use crate::autoname_setup::Setup;
use crate::ui::{SUBTEXT, TEXT};
use anyhow::{Result, bail};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const DAY_MS: u64 = 86_400_000;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Preferences {
    pub enabled: bool,
    pub interval_days: u32,
    /// Unix milliseconds, recorded only after an explicit keep or completed setup.
    pub last_reviewed: BTreeMap<String, u64>,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_days: 90,
            last_reviewed: BTreeMap::new(),
        }
    }
}

impl Preferences {
    pub fn validate_at(&self, now: u64) -> Result<()> {
        if self.interval_days == 0 {
            bail!("reviews.interval_days must be a positive number of days");
        }
        for (id, time) in &self.last_reviewed {
            if !SETTINGS.iter().any(|setting| setting.id == id) {
                bail!("unknown reviewed setting: {id:?}");
            }
            if *time > now {
                bail!("reviews.last_reviewed.{id} is in the future; use Unix milliseconds");
            }
        }
        Ok(())
    }
}

pub struct Values {
    pub current: String,
    pub preset: Option<String>,
}

/// Add an entry here to give an existing settings screen reminders, dates and review UI.
pub struct Setting {
    pub id: &'static str,
    pub label: &'static str,
    pub key: char,
    pub configured: fn(&Config) -> bool,
    pub values: fn(&Config) -> Values,
    pub open_setup: fn(&mut Setup),
}

pub static SETTINGS: &[Setting] = &[Setting {
    id: "auto_name",
    label: "AI names",
    key: 'a',
    configured: |config| config.auto_name.enabled && !config.auto_name.command.is_empty(),
    values: |config| Values {
        current: describe_command(&config.auto_name.command),
        preset: preset_for(&config.auto_name.command).map(|command| describe_command(&command)),
    },
    open_setup: Setup::menu,
}];

pub fn due(config: &Config, now: u64) -> Option<&'static Setting> {
    due_in(SETTINGS, config, now)
}

fn due_in<'a>(settings: &'a [Setting], config: &Config, now: u64) -> Option<&'a Setting> {
    if !config.reviews.enabled {
        return None;
    }
    let interval = u64::from(config.reviews.interval_days) * DAY_MS;
    settings.iter().find(|setting| {
        (setting.configured)(config)
            && config
                .reviews
                .last_reviewed
                .get(setting.id)
                .is_some_and(|last| now.checked_sub(*last).is_some_and(|age| age >= interval))
    })
}

pub fn reviewed(config: &mut Config, id: &str, now: u64) {
    config.reviews.last_reviewed.insert(id.into(), now);
}

fn describe_command(command: &[String]) -> String {
    format!(
        "{}Command (JSON):\n{}",
        config::model_summary(command),
        serde_json::to_string(command).unwrap()
    )
}

fn preset_for(command: &[String]) -> Option<Vec<String>> {
    let executable = std::path::Path::new(command.first()?)
        .file_name()?
        .to_str()?;
    // Exact executable names only; a custom wrapper has no inferred preset.
    let choice = match executable {
        "claude" => '1',
        "codex" => '2',
        "pi" => '3',
        "opencode" => '4',
        "agy" => '5',
        "grok" => '6',
        _ => return None,
    };
    config::preset(choice)
}

pub enum Action {
    Keep,
    Change,
    Hide,
    Close,
}

pub struct Screen {
    pub setting: &'static Setting,
    pub current: String,
    preset: Option<String>,
    scroll: usize,
}

impl Screen {
    pub fn new(setting: &'static Setting, config: &Config) -> Self {
        let values = (setting.values)(config);
        Self {
            setting,
            current: values.current,
            preset: values.preset,
            scroll: 0,
        }
    }

    pub fn key(&mut self, key: KeyEvent) -> Option<Action> {
        match key.code {
            KeyCode::Esc => Some(Action::Close),
            KeyCode::Char('k') if key.modifiers.is_empty() => Some(Action::Keep),
            KeyCode::Char('c') if key.modifiers.is_empty() => Some(Action::Change),
            KeyCode::Char('h') if key.modifiers.is_empty() => Some(Action::Hide),
            KeyCode::Down => {
                self.scroll = self.scroll.saturating_add(1);
                None
            }
            KeyCode::Up => {
                self.scroll = self.scroll.saturating_sub(1);
                None
            }
            _ => None,
        }
    }

    pub fn draw(&mut self, frame: &mut Frame, error: Option<&str>) {
        let [body, footer] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(2)]).areas(frame.area());
        let mut text = format!(
            "Review {}\n\nConfigured now:\n{}\n\nCurrent installed preset:\n{}\n\nPresets come from this installed build.\nNo AI or model lookup runs.\n",
            self.setting.label,
            self.current,
            self.preset
                .as_deref()
                .unwrap_or("No preset for this custom command.")
        );
        if let Some(error) = error {
            text = format!("Error: {error}\n\n{text}");
            self.scroll = 0;
        }
        let lines = wrap(&text, usize::from(body.width));
        self.scroll = self
            .scroll
            .min(lines.len().saturating_sub(usize::from(body.height)));
        frame.render_widget(
            Paragraph::new(
                lines
                    .into_iter()
                    .skip(self.scroll)
                    .map(|line| Line::styled(line, TEXT))
                    .collect::<Vec<_>>(),
            ),
            body,
        );
        frame.render_widget(
            Paragraph::new(vec![
                Line::styled("k keep · c change · h reminders off", SUBTEXT),
                Line::styled("esc close · ↑↓ scroll", SUBTEXT),
            ]),
            footer,
        );
    }
}

/// Hard wrapping keeps every byte of a custom command reachable, without clipping its tail.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for original in text.lines() {
        let mut line = String::new();
        let mut used = 0;
        for c in original.chars() {
            let size = Line::from(c.to_string()).width();
            if used + size > width.max(1) && !line.is_empty() {
                lines.push(std::mem::take(&mut line));
                used = 0;
            }
            line.push(c);
            used += size;
        }
        lines.push(line);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    fn configured() -> Config {
        let mut config = Config::default();
        config.auto_name.enabled = true;
        config.auto_name.command = config::preset('5').unwrap();
        reviewed(&mut config, "auto_name", DAY_MS);
        config
    }
    #[test]
    fn due_not_due_and_exact_boundary_use_the_injected_clock() {
        let mut config = configured();
        assert!(due(&config, 91 * DAY_MS - 1).is_none());
        assert_eq!(due(&config, 91 * DAY_MS).unwrap().id, "auto_name");
        assert!(due(&config, 92 * DAY_MS).is_some());
        config.reviews.interval_days = 7;
        assert!(due(&config, 8 * DAY_MS).is_some());
        reviewed(&mut config, "auto_name", 8 * DAY_MS);
        assert!(due(&config, 8 * DAY_MS).is_none());
    }
    #[test]
    fn never_configured_disabled_and_reminders_turned_off_are_never_due() {
        let mut config = Config::default();
        assert!(due(&config, 200 * DAY_MS).is_none());
        config.auto_name.enabled = true;
        config.auto_name.command = config::preset('5').unwrap();
        assert!(due(&config, 200 * DAY_MS).is_none()); // No invented initial review date.
        config = configured();
        config.auto_name.enabled = false;
        assert!(due(&config, 200 * DAY_MS).is_none());
        config.auto_name.enabled = true;
        config.reviews.enabled = false;
        assert!(due(&config, 200 * DAY_MS).is_none());
    }
    #[test]
    fn malformed_dates_intervals_and_unknown_setting_ids_fail_loudly() {
        let mut prefs = Preferences {
            interval_days: 0,
            ..Preferences::default()
        };
        assert!(prefs.validate_at(1).is_err());
        prefs.interval_days = 90;
        prefs.last_reviewed.insert("auto_name".into(), 2);
        assert!(prefs.validate_at(1).is_err());
        prefs.last_reviewed.clear();
        prefs.last_reviewed.insert("typo".into(), 0);
        assert!(prefs.validate_at(1).is_err());
    }
    #[test]
    fn another_list_entry_gets_due_dates_and_review_values_without_a_dispatch_change() {
        let extra = Setting {
            id: "example",
            label: "Example setting",
            key: 'e',
            configured: |_| true,
            values: |_| Values {
                current: "current value".into(),
                preset: Some("new value".into()),
            },
            open_setup: Setup::menu,
        };
        let mut config = Config::default();
        reviewed(&mut config, "example", DAY_MS);
        let settings = [extra];
        let entry = due_in(&settings, &config, 91 * DAY_MS).unwrap();
        assert_eq!((entry.id, entry.key), ("example", 'e'));
        assert_eq!((entry.values)(&config).current, "current value");
        reviewed(&mut config, entry.id, 91 * DAY_MS);
        assert!(due_in(&settings, &config, 91 * DAY_MS).is_none());
    }
    #[test]
    fn reminder_preferences_round_trip_independently_of_the_setup_hint() {
        let mut config = configured();
        config.reviews.enabled = false;
        config.auto_name.hide_hint = false;
        let bytes = serde_json::to_vec(&config).unwrap();
        let restored: Config = serde_json::from_slice(&bytes).unwrap();
        assert!(!restored.reviews.enabled);
        assert!(!restored.auto_name.hide_hint);
        assert_eq!(restored.reviews.last_reviewed["auto_name"], DAY_MS);
    }

    #[test]
    fn review_commands_and_anchored_actions_are_readable_and_scrollable_at_38_columns() {
        use ratatui::{Terminal, backend::TestBackend};
        let mut config = configured();
        config.auto_name.command[2] = "gemini-old-pin".into();
        let mut review = Screen::new(&SETTINGS[0], &config);
        assert!(review.current.contains("gemini-old-pin"));
        assert!(review.preset.as_ref().unwrap().contains("gemini-3.8-flash"));
        let mut terminal = Terminal::new(TestBackend::new(38, 12)).unwrap();
        let mut seen = String::new();
        for _ in 0..60 {
            terminal.draw(|frame| review.draw(frame, None)).unwrap();
            let buffer = terminal.backend().buffer();
            let text: String = (0..12)
                .map(|y| (0..38).map(|x| buffer[(x, y)].symbol()).collect::<String>() + "\n")
                .collect();
            assert!(text.contains("k keep · c change · h reminders off"));
            assert!(text.contains("esc close · ↑↓ scroll"));
            seen.push_str(&text);
            review.key(KeyEvent::from(KeyCode::Down));
        }
        for value in [
            "gemini-old-pin",
            "Current installed preset:",
            "gemini-3.8-flash",
            "No AI or model lookup runs.",
        ] {
            assert!(seen.contains(value), "missing review value: {value}");
        }
    }

    #[test]
    fn preset_matching_is_explicit_and_custom_commands_have_no_guess() {
        let mut command = config::preset('5').unwrap();
        command[0] = "/usr/local/bin/agy".into();
        command[2] = "old-model-pin".into();
        assert_eq!(preset_for(&command), config::preset('5'));
        assert!(preset_for(&["my-wrapper".into(), "{prompt}".into()]).is_none());
        assert!(
            wrap("wide 界 text and a long command", 8)
                .iter()
                .all(|line| Line::from(line.as_str()).width() <= 8)
        );
    }
}
