//! Shared configuration panel for the sidebar and picker.
mod catalog;
use crate::autoname_config::{self as config, Config, Output};
use crate::index::State;
use crate::ui::{LAVENDER, OVERLAY, SUBTEXT, SURFACE0, SURFACE2, TEXT, colour, fill, hints, panel};
use anyhow::Result;
use catalog::Model;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

#[derive(Default)]
pub struct Setup {
    pub config: Config,
    pub open: bool,
    command: Option<Vec<String>>,
    output: Output,
    preset: Option<crate::index::Kind>,
    custom: Option<String>,
    error: Option<String>,
    config_error: Option<String>,
    /// Presets whose CLI was not on `PATH` when the menu opened.
    missing: Vec<char>,
    selected: usize,
    effort_column: bool,
    notice: Option<String>,
    loading: Option<std::thread::JoinHandle<Result<Vec<Model>>>>,
    choices: Vec<(String, String)>,
    cursor: usize,
    editing: Option<(String, bool, String)>,
    /// Sidebar settings just saved with `t` or `u`, for the sidebar to take up.
    pub saved_sidebar: Option<config::Sidebar>,
}

/// Keep configuration readable in the bottom-left of wider screens.
const KINDS: [&str; 6] = ["claude", "codex", "pi", "opencode", "agy", "grok"];
const CARD_WIDTH: u16 = 60;

impl Setup {
    /// The configuration file is read here, on the key press: nothing else needs it.
    fn open_menu(&mut self) {
        match config::read() {
            Ok(config) => {
                self.config = config;
                self.config_error = None;
            }
            Err(error) => self.config_error = Some(format!("{error:#}")),
        }
        self.missing = CHOICES
            .iter()
            .map(|(choice, _)| *choice)
            .filter(|choice| {
                config::preset_kind(*choice)
                    .is_some_and(|k| config::executable(&self.config.preset_command(k)).is_err())
            })
            .collect();
        self.open = true;
        self.command = None;
        self.custom = None;
        self.error = None;
        self.notice = None;
        self.editing = None;
        self.selected = 0;
        self.effort_column = false;
    }

    /// True consumes the key, so setup Enter never jumps to an agent behind the screen.
    pub fn key(&mut self, key: KeyEvent) -> bool {
        let key = crate::ui::escape_key(key);
        if !self.open {
            if key.modifiers.is_empty() && key.code == KeyCode::Char('o') {
                self.open_menu();
                return true;
            }
            return false;
        }
        if key.code == KeyCode::Esc && self.editing.take().is_some() {
            self.loading = None;
            self.choices.clear();
            self.error = None;
            return true;
        }
        if key.code == KeyCode::Esc {
            if self.custom.take().is_none() && self.command.take().is_none() {
                self.open = false;
            }
            self.error = None;
            return true;
        }
        if let Some((kind, effort, input)) = &mut self.editing {
            let guided = matches!(kind.as_str(), "claude" | "codex" | "agy");
            if guided {
                if self.choices.is_empty() {
                    return true;
                }
                match key.code {
                    KeyCode::Down | KeyCode::Char('j') => {
                        self.cursor = (self.cursor + 1).min(self.choices.len() - 1)
                    }
                    KeyCode::Up | KeyCode::Char('k') => self.cursor = self.cursor.saturating_sub(1),
                    KeyCode::Enter => *input = self.choices[self.cursor].0.clone(),
                    _ => {}
                }
                if key.code != KeyCode::Enter {
                    return true;
                }
            }
            match key.code {
                KeyCode::Enter => {
                    let kind = kind.clone();
                    let effort = *effort;
                    let value = input.trim().to_string();
                    let result = config::read().and_then(|mut cfg| {
                        let previous = self.config.defaults.get(&kind).cloned().unwrap_or_default();
                        let defaults = cfg.defaults.entry(kind).or_default();
                        anyhow::ensure!(
                            !guided || *defaults == previous,
                            "Defaults changed. Reopen Configure to refresh."
                        );

                        if effort {
                            defaults.effort = value;
                        } else {
                            if guided && defaults.model != value {
                                defaults.effort.clear();
                            }
                            defaults.model = value;
                        }
                        config::write(&cfg)?;
                        Ok(cfg)
                    });
                    match result {
                        Ok(cfg) => {
                            self.config = cfg;
                            self.editing = None;
                            self.choices.clear();
                            self.notice = Some("Saved".into());
                        }
                        Err(e) => self.error = Some(format!("{e:#}")),
                    }
                }
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => input.push(c),
                _ => {}
            }
            return true;
        }
        let mut key = key;
        if self.command.is_none() && self.custom.is_none() && key.modifiers.is_empty() {
            match key.code {
                KeyCode::Down | KeyCode::Char('j') => {
                    self.selected = (self.selected + 1).min(17);
                    return true;
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.selected = self.selected.saturating_sub(1);
                    return true;
                }
                KeyCode::Left | KeyCode::Right | KeyCode::Char('h' | 'l')
                    if (11..=16).contains(&self.selected) =>
                {
                    self.effort_column = matches!(key.code, KeyCode::Right | KeyCode::Char('l'));
                    return true;
                }
                KeyCode::Enter | KeyCode::Char(' ') => {
                    let code = match self.selected {
                        0..=6 => KeyCode::Char(char::from(b'1' + self.selected as u8)),
                        7 => {
                            if !self.config.auto_name.enabled
                                && self.config.auto_name.preset.is_none()
                                && self.config.auto_name.command.is_empty()
                            {
                                self.error = Some("Choose a background agent first".into());
                                return true;
                            }
                            let result = config::read().and_then(|mut cfg| {
                                cfg.auto_name.enabled = !cfg.auto_name.enabled;
                                config::write(&cfg)?;
                                Ok(cfg)
                            });
                            match result {
                                Ok(cfg) => {
                                    self.config = cfg;
                                    self.notice = Some("Saved".into());
                                }
                                Err(e) => self.error = Some(format!("{e:#}")),
                            }
                            return true;
                        }
                        8 => KeyCode::Char('t'),
                        9 => KeyCode::Char('u'),
                        10 => KeyCode::Char('p'),
                        11..=16 => {
                            let kind = KINDS[self.selected - 11].to_string();
                            let effort = self.effort_column;
                            let current = self
                                .config
                                .defaults
                                .get(&kind)
                                .map(|d| {
                                    if effort {
                                        d.effort.clone()
                                    } else {
                                        d.model.clone()
                                    }
                                })
                                .unwrap_or_default();
                            self.error = None;
                            self.notice = None;
                            self.choices.clear();
                            self.cursor = 0;
                            let provider = KINDS[self.selected - 11];
                            if matches!(provider, "claude" | "codex" | "agy") {
                                self.loading =
                                    Some(std::thread::spawn(move || catalog::load(provider)));
                            }
                            self.editing = Some((kind, effort, current));
                            return true;
                        }
                        _ => {
                            self.open = false;
                            return true;
                        }
                    };
                    key.code = code;
                }
                _ => {}
            }
        }
        if let Some(input) = &mut self.custom {
            match key.code {
                KeyCode::Enter => {
                    match serde_json::from_str::<Vec<String>>(input)
                        .map_err(anyhow::Error::from)
                        .and_then(|command| {
                            config::executable(&command)?;
                            Ok(command)
                        }) {
                        Ok(command) => {
                            self.command = Some(command);
                            self.preset = None;
                            self.output = Output::Text;
                            self.custom = None;
                            self.error = None;
                            self.notice = None;
                        }
                        Err(error) => self.error = Some(format!("{error:#}")),
                    }
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    input.clear()
                }
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Char(c)
                    if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
                {
                    input.push(c)
                }
                _ => {}
            }
        } else if self.command.is_some() {
            if matches!(key.code, KeyCode::Enter | KeyCode::Char(' ')) && key.modifiers.is_empty() {
                let result: Result<Config> = (|| {
                    let mut config = config::read()?;
                    config.auto_name.preset = self.preset;
                    let reviewed = self.command.as_ref().unwrap();
                    config::executable(reviewed)?;
                    if let Some(kind) = self.preset {
                        anyhow::ensure!(
                            config.preset_command(kind) == *reviewed,
                            "Defaults changed. Reopen Configure to review."
                        );
                        config.auto_name.command.clear();
                    } else {
                        config.auto_name.command = reviewed.clone();
                    }
                    config.auto_name.output = self.output;

                    config::write(&config)?;
                    Ok(config)
                })();
                match result {
                    Ok(config) => {
                        self.notice = Some(format!(
                            "Background agent saved · naming {}",
                            if config.auto_name.enabled {
                                "on"
                            } else {
                                "off"
                            }
                        ));
                        self.config = config;
                        self.command = None;
                        self.selected = 7;
                    }
                    Err(error) => self.error = Some(format!("{error:#}")),
                }
            }
        } else if key.modifiers.is_empty() {
            match key.code {
                KeyCode::Char(choice @ '1'..='6') => {
                    let kind = config::preset_kind(choice).unwrap();
                    let command = self.config.preset_command(kind);
                    match config::executable(&command) {
                        Ok(_) => {
                            self.command = Some(command);
                            self.preset = Some(kind);
                            self.output = if choice == '4' {
                                Output::Opencode
                            } else {
                                Output::Text
                            };
                            self.error = None;
                            self.notice = None;
                        }
                        Err(error) => self.error = Some(format!("{error:#}")),
                    }
                }
                KeyCode::Char('7') => {
                    self.custom = Some(String::new());
                    self.error = None;
                }
                KeyCode::Char(key @ ('t' | 'u' | 'p')) => {
                    // Read afresh to keep whatever another sidebar saved meanwhile.
                    let result = config::read().and_then(|mut config| {
                        let shown = match key {
                            't' => &mut config.sidebar.tasks,
                            'u' => &mut config.sidebar.quota,
                            _ => &mut config.sidebar.family,
                        };
                        *shown = !*shown;
                        config::write(&config)?;
                        Ok(config)
                    });
                    match result {
                        Ok(config) => {
                            self.saved_sidebar = Some(config.sidebar.clone());
                            self.config = config;
                            self.error = None;
                            self.notice = Some("Saved".into());
                        }
                        Err(error) => self.error = Some(format!("{error:#}")),
                    }
                }
                KeyCode::Char('d') if self.config.auto_name.enabled => {
                    // Read afresh to preserve a concurrent setup's command and consent.
                    let result = config::read().and_then(|mut config| {
                        config.auto_name.enabled = false;
                        config::write(&config)?;
                        Ok(config)
                    });
                    match result {
                        Ok(config) => {
                            self.config = config;
                            self.open = false;
                        }
                        Err(error) => self.error = Some(format!("{error:#}")),
                    }
                }
                _ => {}
            }
        }
        true
    }

    fn poll_catalog(&mut self) {
        if self.loading.as_ref().is_some_and(|job| job.is_finished()) {
            let result = self
                .loading
                .take()
                .unwrap()
                .join()
                .unwrap_or_else(|_| Err(anyhow::anyhow!("Catalog lookup stopped")));
            if let Some((kind, effort, current)) = &self.editing {
                match result {
                    Ok(models) => {
                        self.choices.push((
                            String::new(),
                            if *effort {
                                "Automatic".into()
                            } else {
                                "Agent default".into()
                            },
                        ));
                        if *effort {
                            let model = self
                                .config
                                .defaults
                                .get(kind)
                                .map(|d| d.model.as_str())
                                .unwrap_or("");
                            if let Some(selected) = models.iter().find(|m| m.id == model) {
                                self.choices.extend(
                                    selected.efforts.iter().map(|v| (v.clone(), v.clone())),
                                );
                            }
                        } else {
                            self.choices
                                .extend(models.into_iter().map(|m| (m.id, m.name)));
                        }
                        self.cursor = self
                            .choices
                            .iter()
                            .position(|(v, _)| v == current)
                            .unwrap_or(0);
                    }
                    Err(e) => self.error = Some(format!("{e:#}")),
                }
            }
        }
    }

    pub fn draw(&mut self, frame: &mut Frame, area: Rect) {
        self.poll_catalog();
        let width = usize::from(area.width.min(CARD_WIDTH).saturating_sub(4));
        let (mut lines, keys): (Vec<Line>, Vec<(&str, &str)>) =
            if let Some((kind, effort, input)) = &self.editing {
                if matches!(kind.as_str(), "claude" | "codex" | "agy") {
                    let model = self
                        .config
                        .defaults
                        .get(kind)
                        .map(|d| d.model.as_str())
                        .unwrap_or("");
                    let title = if *effort && !model.is_empty() {
                        format!("{model} · effort")
                    } else {
                        format!("{kind} · {}", if *effort { "effort" } else { "model" })
                    };
                    let mut lines = vec![heading(&crate::ui::truncate(&title, width), width)];
                    if self.loading.is_some() {
                        lines.push(muted("Loading available models…"));
                    }
                    if *effort && self.choices.len() == 1 {
                        lines.push(muted(if model.is_empty() {
                            "Choose a model to see effort levels."
                        } else {
                            "No effort levels listed for this model."
                        }));
                    }
                    for (i, (_, label)) in self.choices.iter().enumerate() {
                        let selected = i == self.cursor;
                        lines.push(Line::styled(
                            format!(
                                "{}{}",
                                if selected { "› " } else { "  " },
                                crate::ui::truncate(label, width.saturating_sub(2))
                            ),
                            if selected { LAVENDER } else { TEXT },
                        ));
                    }
                    (lines, vec![("↑↓", "move"), ("⏎", "save"), ("esc", "back")])
                } else {
                    (
                        vec![
                            heading(
                                &format!("{kind} {}", if *effort { "effort" } else { "model" }),
                                width,
                            ),
                            muted("Enter a CLI value; empty uses the agent's own default."),
                            Line::from(format!("› {input}▏")),
                            Line::from("Accept"),
                        ],
                        vec![("⏎", "save"), ("esc", "back")],
                    )
                }
            } else if let Some(input) = &self.custom {
                (
                    custom_lines(input, width),
                    vec![("⏎", "review"), ("^u", "clear"), ("esc", "back")],
                )
            } else if let Some(command) = &self.command {
                (
                    review_lines(command, width),
                    vec![("⏎ space", "accept"), ("esc", "back")],
                )
            } else {
                (
                    self.menu_lines(width),
                    if (11..=16).contains(&self.selected) {
                        vec![
                            ("↑↓", "agent"),
                            ("←→", "field"),
                            ("⏎", "edit"),
                            ("esc", "close"),
                        ]
                    } else {
                        vec![("↑↓ j k", "move"), ("⏎ space", "choose"), ("esc", "close")]
                    },
                )
            };
        if let Some(error) = self.config_error.as_ref().or(self.error.as_ref()) {
            lines.push(Line::default());
            lines.push(Line::styled(format!("⚠ {error}"), colour(State::Blocked)));
        }
        let [panel_area, footer_area] =
            Layout::vertical([Constraint::Min(3), Constraint::Length(2)])
                .areas(card(area, &lines, width));
        let block = panel(Line::from(" configure "));
        let inner = block.inner(panel_area).inner(Margin::new(1, 0));
        frame.render_widget(block, panel_area);
        let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
        let scroll = if self.command.is_none() && self.custom.is_none() && self.editing.is_none() {
            (self.selected + 1 + usize::from(self.selected >= 7) + usize::from(self.selected >= 11))
                .saturating_sub(usize::from(inner.height).saturating_sub(2)) as u16
        } else if !self.choices.is_empty() {
            (self.cursor + 2).saturating_sub(inner.height as usize) as u16
        } else {
            0
        };
        frame.render_widget(paragraph.scroll((scroll, 0)), inner);
        frame.render_widget(
            Paragraph::new(vec![
                hints(&keys),
                Line::styled(self.notice.clone().unwrap_or_default(), colour(State::Done)),
            ]),
            footer_area,
        );
    }

    fn menu_lines(&self, width: usize) -> Vec<Line<'static>> {
        let mut lines = vec![heading(
            if self.config.auto_name.enabled {
                "Background agent · naming on"
            } else {
                "Background agent · naming off"
            },
            width,
        )];
        let mut rows = Vec::new();
        for (choice, label) in CHOICES {
            let current = match config::preset_kind(choice) {
                Some(kind) => self.config.auto_name.preset == Some(kind),
                None => !self.config.auto_name.command.is_empty(),
            };
            rows.push(choice_line(
                choice,
                label,
                current,
                self.missing.contains(&choice),
                width.saturating_sub(2),
            ));
        }
        rows.push(toggle_line(
            ' ',
            "Name agents automatically",
            self.config.auto_name.enabled,
            width.saturating_sub(2),
        ));
        rows.push(toggle_line(
            't',
            "Background tasks",
            self.config.sidebar.tasks,
            width.saturating_sub(2),
        ));
        rows.push(toggle_line(
            'u',
            "Quota panel",
            self.config.sidebar.quota,
            width.saturating_sub(2),
        ));
        rows.push(toggle_line(
            'p',
            "Graph",
            self.config.sidebar.family,
            width.saturating_sub(2),
        ));
        for (i, row) in rows.into_iter().enumerate() {
            if i == 7 {
                lines.push(heading("Preferences", width));
            }
            let mut spans = vec![Span::styled(
                if i == self.selected { "› " } else { "  " },
                LAVENDER,
            )];
            spans.extend(row.spans);
            lines.push(Line::from(spans));
        }
        let model_width = width.saturating_sub(14) * 3 / 5;
        let effort_width = width.saturating_sub(14 + model_width);
        lines.push(Line::from(vec![
            Span::styled("Agent     ", Style::default().fg(LAVENDER).bold()),
            Span::styled(
                format!("  {:<model_width$}", "Model"),
                Style::default().fg(LAVENDER).bold(),
            ),
            Span::styled("  Effort", Style::default().fg(LAVENDER).bold()),
        ]));
        for (i, kind) in KINDS.iter().enumerate() {
            let defaults = self.config.defaults.get(*kind).cloned().unwrap_or_default();
            let mut spans = vec![Span::styled(format!("{kind:<10}"), TEXT)];
            for (effort, value, size) in [
                (false, defaults.model, model_width),
                (true, defaults.effort, effort_width),
            ] {
                let active = self.selected == 11 + i && self.effort_column == effort;
                let style = if active {
                    Style::default().fg(LAVENDER).bg(SURFACE0).bold()
                } else if value.is_empty() {
                    Style::default().fg(SUBTEXT)
                } else {
                    Style::default().fg(colour(State::Done))
                };
                let label =
                    crate::ui::truncate(if value.is_empty() { "default" } else { &value }, size);
                spans.push(Span::styled(
                    format!("{}{label:<size$}", if active { "› " } else { "  " }),
                    style,
                ));
            }
            lines.push(Line::from(spans));
        }
        lines.push(Line::styled(
            if self.selected == 17 {
                "› Done"
            } else {
                "  Done"
            },
            LAVENDER,
        ));
        lines
    }
}

/// `5 Antigravity (agy)` with what is true of it at the right edge.
fn choice_line(
    choice: char,
    label: &str,
    current: bool,
    missing: bool,
    width: usize,
) -> Line<'static> {
    let label_style = if current {
        Style::default().fg(TEXT).bold()
    } else if missing {
        Style::default().fg(OVERLAY)
    } else {
        Style::default().fg(TEXT)
    };
    let left = vec![
        Span::styled(format!("{choice} "), SUBTEXT),
        Span::styled(label.to_string(), label_style),
    ];
    let status = if current {
        Span::styled("✓ saved", colour(State::Done))
    } else if missing {
        Span::styled("not installed", OVERLAY)
    } else {
        return Line::from(left);
    };
    let used = left.iter().map(Span::width).sum::<usize>() + status.width();
    let mut spans = left;
    spans.push(Span::raw(" ".repeat(width.saturating_sub(used).max(1))));
    spans.push(status);
    Line::from(spans)
}

/// `t Background tasks      shown`: a setting with its state at the right edge.
fn toggle_line(key: char, label: &str, shown: bool, width: usize) -> Line<'static> {
    let left = vec![
        Span::styled(format!("{key} "), SUBTEXT),
        Span::styled(label.to_string(), TEXT),
    ];
    let status = if shown {
        Span::styled("[✓]", colour(State::Done))
    } else {
        Span::styled("[ ]", OVERLAY)
    };
    let used = left.iter().map(Span::width).sum::<usize>() + status.width();
    let mut spans = left;
    spans.push(Span::raw(" ".repeat(width.saturating_sub(used).max(1))));
    spans.push(status);
    Line::from(spans)
}

/// Anchor the configuration to the bottom left, growing upward to fit its content.
fn card(area: Rect, lines: &[Line], width: usize) -> Rect {
    let rows: usize = lines
        .iter()
        .map(|line| line.width().div_ceil(width.max(1)).max(1))
        .sum();
    let height = u16::try_from(rows + 4).unwrap_or(u16::MAX).min(area.height);
    Rect {
        x: area.x,
        y: area.bottom().saturating_sub(height),
        width: area.width.min(CARD_WIDTH),
        height,
    }
}

const CHOICES: [(char, &str); 7] = [
    ('1', "Claude Code"),
    ('2', "Codex"),
    ('3', "Pi"),
    ('4', "OpenCode"),
    ('5', "Antigravity (agy)"),
    ('6', "Grok Build"),
    ('7', "Custom command"),
];

/// The sidebar's session heading: a coloured name and a rule out to the edge.
fn heading(text: &str, width: usize) -> Line<'static> {
    let rule = width.saturating_sub(text.chars().count() + 1);
    Line::from(vec![
        Span::styled(text.to_string(), Style::default().fg(LAVENDER).bold()),
        Span::styled(format!(" {}", "─".repeat(rule)), SURFACE2),
    ])
}

fn muted(text: &str) -> Line<'static> {
    Line::styled(text.to_string(), OVERLAY)
}

fn review_lines(command: &[String], width: usize) -> Vec<Line<'static>> {
    let mut lines = vec![heading(&command[0], width)];
    for row in config::model_summary(command).lines() {
        lines.push(match row.split_once(": ") {
            Some((label, value)) => Line::from(vec![
                Span::styled(format!("{label:<9} "), OVERLAY),
                Span::styled(value.to_string(), TEXT),
            ]),
            None => muted(row),
        });
    }
    lines.push(Line::default());
    lines.push(heading("What is sent", width));
    for item in [
        "Task title",
        "Working directory",
        "Blocked detail",
        "Cleanup (z): task commands",
    ] {
        lines.push(Line::from(vec![
            Span::styled("· ", OVERLAY),
            Span::styled(item, TEXT),
        ]));
    }
    lines.push(muted("Its AI provider may receive these."));
    lines.push(muted("Never screens or file contents."));
    lines.push(Line::default());
    lines.push(heading("Naming", width));
    lines.push(muted("One attempt per agent pane. Names you set are kept."));
    lines.push(Line::default());
    lines.push(heading("Command", width));
    lines.push(muted(&serde_json::to_string(command).unwrap()));
    lines
}

fn custom_lines(input: &str, width: usize) -> Vec<Line<'static>> {
    // Show the tail of long input so typing stays reviewable at 38 columns.
    let tail: String = input.chars().rev().take(120).collect();
    let tail: String = tail.chars().rev().collect();
    let field = Line::from(vec![
        Span::styled("› ", LAVENDER),
        Span::styled(format!("{tail}▏"), TEXT),
    ])
    .style(Style::default().bg(SURFACE0));
    vec![
        heading("Custom command", width),
        muted("A JSON array: executable, then arguments. One argument must be \"{prompt}\"."),
        muted("It has 60s to return name and orchestrator as JSON."),
        Line::default(),
        heading("Example", width),
        muted("[\"/path/to/namer\",\"{prompt}\"]"),
        Line::default(),
        heading("Your command", width),
        fill(field, width),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn navigation_selects_done_and_editors_close_with_control_c() {
        let mut setup = Setup {
            open: true,
            ..Setup::default()
        };
        for _ in 0..17 {
            setup.key(KeyEvent::from(KeyCode::Down));
        }
        let text = rendered(&mut setup, 38, 20);
        assert!(text.contains("› Done"), "{text}");
        setup.key(KeyEvent::from(KeyCode::Enter));
        assert!(!setup.open);
        setup.open = true;
        setup.selected = 13;
        setup.key(KeyEvent::from(KeyCode::Char(' ')));
        assert!(setup.editing.is_some());
        setup.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(setup.open && setup.editing.is_none());
        setup.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        assert!(!setup.open);
    }

    #[test]
    fn o_opens_configuration_whatever_the_state() {
        for enabled in [false, true] {
            let mut setup = Setup::default();
            setup.config.auto_name.enabled = enabled;
            assert!(setup.key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::NONE)));
            assert!(setup.open);
        }
        let mut setup = Setup::default();
        assert!(!setup.key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL)));
        assert!(!setup.key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE)));
    }

    #[test]
    fn setup_consumes_enter_and_cancel_without_leaking_navigation() {
        let mut setup = Setup {
            open: true,
            ..Setup::default()
        };
        assert!(setup.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)));
        assert!(setup.open);
        assert!(setup.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)));
        // Enter opens a submenu only when the selected CLI is installed.
        // Test Ctrl+C from an open menu independently of that host state.
        setup.open = true;
        assert!(setup.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)));
        assert!(!setup.open);
        assert!(!setup.key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE)));
    }
    /// One screen of a setup, as text.
    fn rendered(setup: &mut Setup, width: u16, height: u16) -> String {
        use ratatui::{Terminal, backend::TestBackend};
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| setup.draw(frame, frame.area()))
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    + "\n"
            })
            .collect()
    }

    #[test]
    fn menu_distinguishes_the_saved_command_from_naming_and_missing_clis() {
        let mut setup = Setup {
            open: true,
            missing: vec!['2'],
            ..Setup::default()
        };
        setup.config.auto_name.enabled = true;
        setup.config.auto_name.preset = config::preset_kind('1');
        let text = rendered(&mut setup, 38, 28);
        assert!(text.contains("Background agent"));
        assert!(text.contains("1 Claude Code") && text.contains("✓ saved"));
        assert!(text.contains("Background agent · naming on"));
        assert!(text.contains("2 Codex") && text.contains("not installed"));
        assert!(text.contains("Name agents automatically") && text.contains("[✓]"));
        assert!(text.contains("Model") && text.contains("Effort"));
        assert!(
            text.lines()
                .any(|row| row.contains("claude") && row.matches("default").count() == 2)
        );
        setup.selected = 13;
        setup.key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        setup.key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(
            setup.editing.take(),
            Some(("pi".into(), true, String::new()))
        );
        setup.config.auto_name.enabled = false;
        let text = rendered(&mut setup, 38, 28);
        assert!(text.contains("Background agent · naming off"));
        assert!(text.contains("✓ saved"));
    }

    #[test]
    fn a_wide_screen_shows_a_centred_card_with_nothing_cut_off() {
        for (command, custom) in [
            (None, None),
            (
                Some(Config::default().preset_command(crate::index::Kind::Claude)),
                None,
            ),
            (None, Some(String::new())),
        ] {
            let mut setup = Setup {
                open: true,
                command,
                custom,
                ..Setup::default()
            };
            let text = rendered(&mut setup, 120, 40);
            let rows: Vec<&str> = text.lines().collect();
            let top = rows
                .iter()
                .position(|row| row.contains("╭ configure"))
                .unwrap();
            assert!(top > 0, "anchored at bottom");
            assert_eq!(rows[top].chars().position(|c| c == '╭'), Some(0));
            let bottom = rows.iter().position(|row| row.contains('╰')).unwrap();
            assert!(rows[bottom + 1].contains("esc"), "hints sit under the card");
            // The last line of content shows, with room to spare for word wrap.
            assert_eq!(bottom + 3, rows.len());
        }
    }

    #[test]
    fn setup_fits_a_38_column_sidebar() {
        use ratatui::{Terminal, backend::TestBackend};
        let mut setup = Setup {
            open: true,
            command: Some(Config::default().preset_command(crate::index::Kind::Agy)),
            ..Setup::default()
        };
        let mut terminal = Terminal::new(TestBackend::new(38, 28)).unwrap();
        terminal
            .draw(|frame| setup.draw(frame, frame.area()))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let text: String = (0..28)
            .map(|y| (0..38).map(|x| buffer[(x, y)].symbol()).collect::<String>() + "\n")
            .collect();
        assert!(text.contains("accept"));
        assert!(text.contains("Model     agent default"));
        assert!(text.contains("Thinking  automatic"));
        assert!(text.contains("What is sent"));
        assert!(text.contains("Its AI provider may receive"));
        assert!(text.contains("╭ configure"));
    }
}
