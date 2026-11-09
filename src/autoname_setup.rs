//! Sidebar-local setup; no model process runs during configuration.
use crate::autoname_config::{self as config, Config, Output};
use crate::index::State;
use crate::ui::{LAVENDER, OVERLAY, SUBTEXT, SURFACE0, SURFACE2, TEXT, colour, fill, hints, panel};
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
    /// Presets whose CLI was not on `PATH` when the menu opened.
    missing: Vec<char>,
    /// Sidebar settings just saved with `t` or `u`, for the sidebar to take up.
    pub saved_sidebar: Option<config::Sidebar>,
}

/// A wider screen, such as the picker's, shows the setup as a centred card of this width.
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
                config::preset(*choice).is_some_and(|c| config::executable(&c).is_err())
            })
            .collect();
        self.open = true;
        self.command = None;
        self.custom = None;
        self.error = None;
    }

    /// True consumes the key, so setup Enter never jumps to an agent behind the screen.
    pub fn key(&mut self, key: KeyEvent) -> bool {
        if !self.open {
            if key.modifiers.is_empty() && key.code == KeyCode::Char('o') {
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

    pub fn draw(&mut self, frame: &mut Frame, area: Rect) {
        let width = usize::from(area.width.min(CARD_WIDTH).saturating_sub(4));
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
            (
                self.menu_lines(width),
                vec![("1-7", "names"), ("t u", "sidebar"), ("esc", "close")],
            )
        };
        if let Some(error) = self.config_error.as_ref().or(self.error.as_ref()) {
            lines.push(Line::default());
            lines.push(Line::styled(format!("⚠ {error}"), colour(State::Blocked)));
        }
        let [panel_area, footer_area] =
            Layout::vertical([Constraint::Min(3), Constraint::Length(1)])
                .areas(card(area, &lines, width));
        let block = panel(Line::from(" configure "));
        let inner = block.inner(panel_area).inner(Margin::new(1, 0));
        frame.render_widget(block, panel_area);
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        frame.render_widget(Paragraph::new(hints(&keys)), footer_area);
    }

    fn menu_lines(&self, width: usize) -> Vec<Line<'static>> {
        let auto_name = &self.config.auto_name;
        let is_current = |choice| {
            auto_name.enabled && config::preset(choice).as_ref() == Some(&auto_name.command)
        };
        let any_preset = CHOICES.iter().any(|(choice, _)| is_current(*choice));
        let state = if auto_name.enabled {
            Span::styled("● on", colour(State::Done))
        } else {
            Span::styled("○ off", OVERLAY)
        };
        let mut lines = vec![heading_with("AI agent names", state, width)];
        lines.push(muted(
            "Name agents after their task, using an AI CLI on this computer.",
        ));
        lines.push(Line::default());
        for (choice, label) in CHOICES {
            let current = is_current(choice) || (choice == '7' && auto_name.enabled && !any_preset);
            let missing = self.missing.contains(&choice);
            lines.push(choice_line(choice, label, current, missing, width));
        }
        lines.push(Line::default());
        lines.push(muted(
            "You review each choice before anything runs. Presets use a low-cost model.",
        ));
        if auto_name.enabled {
            lines.push(muted("d turns AI names off."));
        }
        lines.push(Line::default());
        lines.push(heading("Sidebar", width));
        let sidebar = &self.config.sidebar;
        lines.push(toggle_line('t', "Background tasks", sidebar.tasks, width));
        lines.push(toggle_line('u', "Quota panel", sidebar.quota, width));
        lines.push(toggle_line('p', "Family panel", sidebar.family, width));
        lines.push(muted("What every sidebar shows when it opens."));
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
        Span::styled("✓ active", colour(State::Done))
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
        Span::styled("● shown", colour(State::Done))
    } else {
        Span::styled("○ hidden", OVERLAY)
    };
    let used = left.iter().map(Span::width).sum::<usize>() + status.width();
    let mut spans = left;
    spans.push(Span::raw(" ".repeat(width.saturating_sub(used).max(1))));
    spans.push(status);
    Line::from(spans)
}

/// The whole area in a narrow pane; otherwise a centred card just tall enough for `lines`.
fn card(area: Rect, lines: &[Line], width: usize) -> Rect {
    if area.width <= CARD_WIDTH {
        return area;
    }
    let rows: usize = lines
        .iter()
        .map(|line| line.width().div_ceil(width).max(1))
        .sum();
    // Borders and hints, and a row for each of the few long lines that wrap at a word.
    let height = u16::try_from(rows + 5).unwrap_or(u16::MAX).min(area.height);
    Rect {
        x: area.x + (area.width - CARD_WIDTH) / 2,
        y: area.y + (area.height - height) / 2,
        width: CARD_WIDTH,
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

/// A heading whose rule ends in `state`.
fn heading_with(text: &str, state: Span<'static>, width: usize) -> Line<'static> {
    let rule = width.saturating_sub(text.chars().count() + state.width() + 2);
    Line::from(vec![
        Span::styled(text.to_string(), Style::default().fg(LAVENDER).bold()),
        Span::styled(format!(" {} ", "─".repeat(rule)), SURFACE2),
        state,
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
    for item in ["Task title", "Working directory", "Blocked detail"] {
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
        muted("It runs with no tools and has 60s to print a plain name."),
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
    fn menu_marks_the_active_command_and_missing_clis() {
        let mut setup = Setup {
            open: true,
            missing: vec!['2'],
            ..Setup::default()
        };
        setup.config.auto_name.enabled = true;
        setup.config.auto_name.command = config::preset('1').unwrap();
        let text = rendered(&mut setup, 38, 28);
        assert!(
            text.lines()
                .any(|l| l.contains("AI agent names") && l.contains("● on"))
        );
        assert!(
            text.lines()
                .any(|l| l.contains("1 Claude Code") && l.contains("✓ active"))
        );
        assert!(
            text.lines()
                .any(|l| l.contains("2 Codex") && l.contains("not installed"))
        );
        assert!(text.contains("1-7 names"));
        assert!(text.contains("d turns AI names off"));
        assert!(
            text.lines()
                .any(|l| l.contains("t Background tasks") && l.contains("● shown"))
        );
        assert!(
            text.lines()
                .any(|l| l.contains("u Quota panel") && l.contains("○ hidden"))
        );
    }

    #[test]
    fn a_wide_screen_shows_a_centred_card_with_nothing_cut_off() {
        for (command, custom) in [
            (None, None),
            (config::preset('1'), None),
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
            assert!(top > 0, "vertically centred");
            assert_eq!(rows[top].chars().position(|c| c == '╭'), Some(30));
            let bottom = rows.iter().position(|row| row.contains('╰')).unwrap();
            assert!(rows[bottom + 1].contains("esc"), "hints sit under the card");
            // The last line of content shows, with room to spare for word wrap.
            assert!(rows[bottom - 1].chars().all(|c| c == ' ' || c == '│'));
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
        terminal
            .draw(|frame| setup.draw(frame, frame.area()))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let text: String = (0..28)
            .map(|y| (0..38).map(|x| buffer[(x, y)].symbol()).collect::<String>() + "\n")
            .collect();
        assert!(text.contains("⏎ enable"));
        assert!(text.contains("Model     gemini-3.8-flash"));
        assert!(text.contains("Thinking  low"));
        assert!(text.contains("What is sent"));
        assert!(text.contains("Its AI provider may receive"));
        assert!(text.contains("╭ configure"));
    }
}
