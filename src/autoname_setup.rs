//! Sidebar-local setup; no model process runs during configuration.
use crate::autoname_config::{self as config, Config, Output};
use crate::index::State;
use crate::ui::{LAVENDER, OVERLAY, SUBTEXT, SURFACE0, SURFACE2, TEXT, colour, hints, panel};
use anyhow::Result;
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
    custom: Option<String>,
    error: Option<String>,
    config_error: Option<String>,
}

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
        self.open = true;
        self.command = None;
        self.custom = None;
        self.error = None;
    }

    /// True consumes the key, so setup Enter never jumps to an agent behind the screen.
    pub fn key(&mut self, key: KeyEvent) -> bool {
        if !self.open {
            if key.modifiers.is_empty() && key.code == KeyCode::Char('c') {
                self.open_menu();
                return true;
            }
            return false;
        }
        if key.code == KeyCode::Esc {
            if self.custom.take().is_none() && self.command.take().is_none() {
                self.open = false;
            }
            self.error = None;
            return true;
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
                            self.output = Output::Text;
                            self.custom = None;
                            self.error = None;
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
            if key.code == KeyCode::Enter && key.modifiers.is_empty() {
                let result: Result<Config> = (|| {
                    let mut config = config::read()?;
                    config.auto_name.enabled = true;
                    config.auto_name.command = self.command.clone().unwrap();
                    config.auto_name.output = self.output;
                    config::executable(&config.auto_name.command)?;
                    config::write(&config)?;
                    Ok(config)
                })();
                match result {
                    Ok(config) => {
                        self.config = config;
                        self.open = false;
                        self.command = None;
                    }
                    Err(error) => self.error = Some(format!("{error:#}")),
                }
            }
        } else if key.modifiers.is_empty() {
            match key.code {
                KeyCode::Char(choice @ '1'..='6') => {
                    let command = config::preset(choice).unwrap();
                    match config::executable(&command) {
                        Ok(_) => {
                            self.command = Some(command);
                            self.output = if choice == '4' {
                                Output::Opencode
                            } else {
                                Output::Text
                            };
                            self.error = None;
                        }
                        Err(error) => self.error = Some(format!("{error:#}")),
                    }
                }
                KeyCode::Char('7') => {
                    self.custom = Some(String::new());
                    self.error = None;
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

    pub fn draw(&mut self, frame: &mut Frame, area: Rect) {
        let [panel_area, footer_area] =
            Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(area);
        let enabled = self.config.auto_name.enabled;
        let state = if enabled {
            Line::styled(" on ", colour(State::Done)).right_aligned()
        } else {
            Line::styled(" off ", OVERLAY).right_aligned()
        };
        let block = panel(Line::from(" configure ")).title_top(state);
        let inner = block.inner(panel_area).inner(Margin::new(1, 0));
        frame.render_widget(block, panel_area);
        let width = inner.width as usize;
        let (mut lines, keys): (Vec<Line>, Vec<(&str, &str)>) = if let Some(input) = &self.custom {
            (
                custom_lines(input, width),
                vec![("⏎", "review"), ("^u", "clear"), ("esc", "back")],
            )
        } else if let Some(command) = &self.command {
            (
                review_lines(command, width),
                vec![("⏎", "enable"), ("esc", "back")],
            )
        } else {
            let keys = if enabled {
                vec![("d", "turn off"), ("esc", "close")]
            } else {
                vec![("esc", "close")]
            };
            (self.menu_lines(width), keys)
        };
        if let Some(error) = self.config_error.as_ref().or(self.error.as_ref()) {
            lines.push(Line::default());
            lines.push(Line::styled(error.clone(), colour(State::Blocked)));
        }
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        frame.render_widget(Paragraph::new(hints(&keys)), footer_area);
    }

    fn menu_lines(&self, width: usize) -> Vec<Line<'static>> {
        let auto_name = &self.config.auto_name;
        let is_current = |choice| {
            auto_name.enabled && config::preset(choice).as_ref() == Some(&auto_name.command)
        };
        let any_preset = CHOICES.iter().any(|(choice, _)| is_current(*choice));
        let mut lines = vec![heading("AI agent names", width)];
        lines.push(muted(
            "Name default-named agents with an AI CLI on your computer.",
        ));
        lines.push(Line::default());
        for (choice, label) in CHOICES {
            let current = is_current(choice) || (choice == '7' && auto_name.enabled && !any_preset);
            lines.push(Line::from(vec![
                Span::styled(format!("{choice} "), SUBTEXT),
                Span::styled(label, TEXT),
                Span::styled(if current { " ✓" } else { "" }, colour(State::Done)),
            ]));
        }
        lines.push(Line::default());
        lines.push(muted(
            "Nothing runs until you review and enable. Presets use a low-cost model at its lowest thinking.",
        ));
        lines
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
                Span::styled(format!("{label}: "), OVERLAY),
                Span::styled(value.to_string(), TEXT),
            ]),
            None => muted(row),
        });
    }
    lines.push(Line::default());
    lines.push(heading("What is sent", width));
    lines.push(Line::styled(
        "Title, cwd and blocked detail.".to_string(),
        TEXT,
    ));
    lines.push(muted(
        "Its AI provider may receive that data. No screens or file contents.",
    ));
    lines.push(Line::default());
    lines.push(heading("Naming", width));
    lines.push(muted(
        "One attempt per agent pane lifetime. Manual and --name names stay yours.",
    ));
    lines.push(Line::default());
    lines.push(heading("Command", width));
    lines.push(muted(&serde_json::to_string(command).unwrap()));
    lines
}

fn custom_lines(input: &str, width: usize) -> Vec<Line<'static>> {
    // Show the tail of long input so typing stays reviewable at 38 columns.
    let tail: String = input.chars().rev().take(120).collect();
    let tail: String = tail.chars().rev().collect();
    vec![
        heading("Custom command", width),
        muted("A JSON array: executable, then arguments. One argument must be \"{prompt}\"."),
        muted("It runs with no tools and 60s to print a plain name to stdout."),
        Line::default(),
        muted("Example:"),
        muted("[\"/path/to/namer\",\"{prompt}\"]"),
        Line::default(),
        Line::styled(format!("{tail}▏"), Style::default().fg(TEXT).bg(SURFACE0)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn c_opens_configuration_whatever_the_state() {
        for enabled in [false, true] {
            let mut setup = Setup::default();
            setup.config.auto_name.enabled = enabled;
            assert!(setup.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE)));
            assert!(setup.open);
        }
        let mut setup = Setup::default();
        assert!(!setup.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)));
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
        assert!(!setup.open);
        assert!(!setup.key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE)));
    }
    #[test]
    fn setup_fits_a_38_column_sidebar() {
        use ratatui::{Terminal, backend::TestBackend};
        let mut setup = Setup {
            open: true,
            command: config::preset('5'),
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
        assert!(text.contains("⏎ enable"));
        assert!(text.contains("Model: gemini-3.8-flash"));
        assert!(text.contains("Thinking: low"));
        assert!(text.contains("What is sent"));
        assert!(text.contains("Its AI provider may receive"));
        assert!(text.contains("╭ configure"));
    }
}
