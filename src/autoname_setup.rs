//! Sidebar-local setup; no model process runs during configuration.
use crate::autoname_config::{self as config, Config, Output};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::{
    Frame,
    style::{Color, Style},
};

#[derive(Default)]
pub struct Setup {
    pub config: Config,
    pub open: bool,
    command: Option<Vec<String>>,
    output: Output,
    custom: Option<String>,
    error: Option<String>,
}

impl Setup {
    /// Called once on sidebar startup and when deliberately opening setup, never on a timer.
    pub fn load(&mut self) {
        match config::read() {
            Ok(config) => {
                self.config = config;
                self.error = None;
            }
            Err(error) => self.error = Some(format!("{error:#}")),
        }
    }

    pub fn hint(&self) -> bool {
        !self.config.auto_name.enabled && !self.config.auto_name.hide_hint
    }

    /// True consumes the key, so setup Enter never jumps to an agent behind the screen.
    pub fn key(&mut self, key: KeyEvent) -> bool {
        if !self.open {
            if key.code != KeyCode::Char('a') || !key.modifiers.is_empty() {
                return false;
            }
            self.load();
            self.open = true;
            self.command = None;
            self.custom = None;
            return true;
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
                let mut config = self.config.clone();
                config.auto_name.enabled = true;
                config.auto_name.command = self.command.clone().unwrap();
                config.auto_name.output = self.output;
                match config::executable(&config.auto_name.command)
                    .and_then(|_| config::write(&config))
                {
                    Ok(()) => {
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
                KeyCode::Char('h' | 'd') => {
                    // Read afresh to preserve a concurrent setup's command and consent.
                    let result = config::read().and_then(|mut config| {
                        if key.code == KeyCode::Char('h') {
                            config.auto_name.hide_hint = true;
                        } else {
                            config.auto_name.enabled = false;
                        }
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

    pub fn draw(&self, frame: &mut Frame) {
        let mut text = format!(
            "AI auto-naming · {}\n\n",
            if self.config.auto_name.enabled {
                "on"
            } else {
                "off"
            }
        );
        if let Some(input) = &self.custom {
            text.push_str("Custom noninteractive command\n\nJSON array: executable + arguments.\nOne argument must be \"{prompt}\".\nPlain name on stdout; no tools.\nRuns with a 60s timeout.\n\nExample:\n[\"/path/to/namer\",\"{prompt}\"]\n\n");
            // Show the tail of long input so typing stays reviewable at 38 columns.
            let tail: String = input.chars().rev().take(120).collect();
            text.push_str(&tail.chars().rev().collect::<String>());
            text.push_str("▏\n\nenter review · ctrl-u clear\nesc back");
        } else if let Some(command) = &self.command {
            text.push_str(&config::model_summary(command));
            text.push_str("\nSends title, cwd and blocked detail.\nIts AI provider may receive that data.\nWe pass no screens or file contents.\n\nOne attempt per agent pane lifetime.\nManual and --name names stay yours.\n\nCommand (JSON):\n");
            text.push_str(&serde_json::to_string(command).unwrap());
            text.push_str("\n\nenter enable · esc back");
        } else {
            text.push_str("Choose the AI command you use:\n\n1 Claude Code\n2 Codex\n3 Pi\n4 OpenCode\n5 Antigravity (agy)\n6 Grok Build\n7 Custom command\n\nh Hide setup hint permanently\nd Disable auto-naming\nesc Close\n\nNo AI runs until you review and\nenable. Your CLI's authentication\napplies. Presets select a low-cost\nmodel and its lowest verified effort.");
        }
        if let Some(error) = &self.error {
            text.push_str(&format!("\n\nError: {error}"));
        }
        frame.render_widget(
            Paragraph::new(text)
                .style(Style::default().fg(Color::Rgb(205, 214, 244)))
                .wrap(Wrap { trim: false }),
            frame.area(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hint_is_independent_of_visibility_and_consent() {
        let mut setup = Setup::default();
        assert!(setup.hint());
        setup.config.auto_name.hide_hint = true;
        assert!(!setup.hint());
        setup.config.auto_name.hide_hint = false;
        setup.config.auto_name.enabled = true;
        assert!(!setup.hint());
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
        let setup = Setup {
            open: true,
            command: config::preset('5'),
            ..Setup::default()
        };
        let mut terminal = Terminal::new(TestBackend::new(38, 28)).unwrap();
        terminal.draw(|frame| setup.draw(frame)).unwrap();
        let buffer = terminal.backend().buffer();
        let text: String = (0..28)
            .map(|y| (0..38).map(|x| buffer[(x, y)].symbol()).collect::<String>() + "\n")
            .collect();
        assert!(text.contains("enter enable"));
        assert!(text.contains("Model: gemini-3.8-flash"));
        assert!(text.contains("Thinking: low"));
        assert!(text.contains("AI provider may receive that data"));
    }
}
