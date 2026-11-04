//! Sidebar-local setup; no model process runs during configuration.
use crate::autoname_config::{self as config, Config, Output};
use crate::{index, settings_review as reviews};
use anyhow::{Context, Result, bail};
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
    config_error: Option<String>,
    due: Option<&'static str>,
    active: Option<&'static reviews::Setting>,
    review: Option<reviews::Screen>,
}

impl Setup {
    /// Read on the sidebar's existing slow refresh. The return value lets callers redraw
    /// only when configuration, errors or due state changed, including the time boundary.
    pub fn refresh(&mut self) -> bool {
        match config::read() {
            Ok(config) => {
                let error_changed = self.config_error.take().is_some();
                self.accept_config(config, index::now_ms()) || error_changed
            }
            Err(error) => {
                let message = format!("{error:#}");
                let changed = self.config_error.as_ref() != Some(&message) || self.due.is_some();
                self.config_error = Some(message);
                self.due = None;
                changed
            }
        }
    }

    fn accept_config(&mut self, config: Config, now: u64) -> bool {
        let due = reviews::due(&config, now).map(|setting| setting.id);
        let changed = self.config != config || self.due != due;
        self.config = config;
        self.due = due;
        changed
    }

    pub fn load(&mut self) {
        self.refresh();
    }

    pub fn flags(&self) -> Vec<String> {
        let mut flags = Vec::new();
        if self.hint() {
            flags.push(" a AI names · optional setup".into());
        }
        if let Some(setting) = reviews::SETTINGS
            .iter()
            .find(|setting| Some(setting.id) == self.due)
        {
            flags.push(format!(" {} {} · review due", setting.key, setting.label));
        }
        flags
    }

    pub(crate) fn menu(&mut self) {
        self.open = true;
        self.review = None;
        self.command = None;
        self.custom = None;
        self.error = None;
    }

    fn review_action(&mut self, action: reviews::Action) {
        let setting = self.review.as_ref().unwrap().setting;
        match action {
            reviews::Action::Close => {
                self.review = None;
                self.open = false;
            }
            reviews::Action::Change => (setting.open_setup)(self),
            reviews::Action::Keep | reviews::Action::Hide => {
                let result: Result<Config> = (|| {
                    let mut config = config::read()?;
                    if matches!(action, reviews::Action::Keep) {
                        let current = (setting.values)(&config).current;
                        if !(setting.configured)(&config)
                            || current != self.review.as_ref().unwrap().current
                        {
                            bail!("settings changed; close and review again");
                        }
                        reviews::reviewed(&mut config, setting.id, index::now_ms());
                    } else {
                        config.reviews.enabled = false;
                    }
                    config::write(&config)?;
                    Ok(config)
                })();
                match result {
                    Ok(config) => {
                        self.accept_config(config, index::now_ms());
                        self.review = None;
                        self.open = false;
                        self.error = None;
                    }
                    Err(error) => self.error = Some(format!("{error:#}")),
                }
            }
        }
    }

    pub fn hint(&self) -> bool {
        !self.config.auto_name.enabled && !self.config.auto_name.hide_hint
    }

    /// True consumes the key, so setup Enter never jumps to an agent behind the screen.
    pub fn key(&mut self, key: KeyEvent) -> bool {
        if !self.open {
            if !key.modifiers.is_empty() {
                return false;
            }
            let KeyCode::Char(c) = key.code else {
                return false;
            };
            let Some(setting) = reviews::SETTINGS.iter().find(|setting| setting.key == c) else {
                return false;
            };
            self.load();
            self.active = Some(setting);
            self.error = None;
            if self.due == Some(setting.id) {
                self.open = true;
                self.review = Some(reviews::Screen::new(setting, &self.config));
            } else {
                (setting.open_setup)(self);
            }
            return true;
        }
        if let Some(review) = &mut self.review {
            if let Some(action) = review.key(key) {
                self.review_action(action);
            }
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
                let result: Result<Config> = (|| {
                    let mut config = config::read()?;
                    config.auto_name.enabled = true;
                    config.auto_name.command = self.command.clone().unwrap();
                    config.auto_name.output = self.output;
                    let setting = self.active.context("setup has no reviewable setting")?;
                    reviews::reviewed(&mut config, setting.id, index::now_ms());
                    config::executable(&config.auto_name.command)?;
                    config::write(&config)?;
                    Ok(config)
                })();
                match result {
                    Ok(config) => {
                        self.accept_config(config, index::now_ms());
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
                            let setting = self.active.context("setup has no reviewable setting")?;
                            if (setting.configured)(&config) {
                                reviews::reviewed(&mut config, setting.id, index::now_ms());
                            }
                            config.auto_name.enabled = false;
                        }
                        config::write(&config)?;
                        Ok(config)
                    });
                    match result {
                        Ok(config) => {
                            self.accept_config(config, index::now_ms());
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

    pub fn draw(&mut self, frame: &mut Frame) {
        if let Some(review) = &mut self.review {
            review.draw(
                frame,
                self.config_error.as_deref().or(self.error.as_deref()),
            );
            return;
        }
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
        if let Some(error) = self.config_error.as_ref().or(self.error.as_ref()) {
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
    fn refresh_reports_boundary_and_shared_review_date_changes_without_a_new_timer() {
        let day = reviews::DAY_MS;
        let mut config = Config::default();
        config.auto_name.enabled = true;
        config.auto_name.command = config::preset('5').unwrap();
        reviews::reviewed(&mut config, "auto_name", day);
        let mut one = Setup::default();
        let mut two = Setup::default();
        assert!(one.accept_config(config.clone(), 91 * day - 1));
        assert!(!one.accept_config(config.clone(), 91 * day - 1));
        assert!(one.accept_config(config.clone(), 91 * day));
        assert!(one.flags()[0].contains("review due"));
        assert!(
            one.flags()
                .iter()
                .all(|flag| ratatui::text::Line::from(flag.as_str()).width() <= 38)
        );
        two.accept_config(config.clone(), 91 * day);
        assert_eq!(one.flags(), two.flags());
        reviews::reviewed(&mut config, "auto_name", 91 * day);
        assert!(one.accept_config(config.clone(), 91 * day));
        assert!(two.accept_config(config, 91 * day));
        assert!(one.flags().is_empty() && two.flags().is_empty());
    }

    #[test]
    fn leaving_a_review_or_change_without_saving_keeps_it_due() {
        let day = reviews::DAY_MS;
        let mut config = Config::default();
        config.auto_name.enabled = true;
        config.auto_name.command = config::preset('5').unwrap();
        reviews::reviewed(&mut config, "auto_name", day);
        for action in [reviews::Action::Close, reviews::Action::Change] {
            let mut setup = Setup::default();
            setup.accept_config(config.clone(), 91 * day);
            setup.open = true;
            setup.active = Some(&reviews::SETTINGS[0]);
            setup.review = Some(reviews::Screen::new(&reviews::SETTINGS[0], &config));
            setup.review_action(action);
            assert_eq!(setup.config.reviews.last_reviewed["auto_name"], day);
            assert!(setup.flags()[0].contains("review due"));
            if setup.open {
                setup.key(KeyEvent::from(KeyCode::Esc));
            }
            assert!(!setup.open);
            assert_eq!(setup.config, config);
        }
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
