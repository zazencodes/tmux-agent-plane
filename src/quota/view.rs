//! The sidebar's quota view (`u`): full height in place of the agent list. It reads quota.json
//! and asks the daemon for a fresh read when opened, every five minutes while open, and on `r`;
//! the request waits on its own thread, so the sidebar stays responsive.

use super::{ProviderId, Snapshot, Status, render};
use crate::index::State;
use crate::ui::{OVERLAY, colour, hints, panel, truncate};
use anyhow::Result;
use chrono::Utc;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};

const EVERY: Duration = Duration::from_secs(300);

#[derive(Default)]
pub struct View {
    pub open: bool,
    snapshot: Option<Snapshot>,
    /// This sidebar's request, while the daemon works on it.
    request: Option<Receiver<Result<()>>>,
    requested_at: Option<Instant>,
    /// Why the last request or read failed.
    notice: Option<String>,
    scroll: usize,
    /// Lines that fit, from the last draw.
    height: usize,
    lines: usize,
}

impl View {
    /// While open, every key but `?` is the view's. `u` opens it when nothing else is typing.
    pub fn key(&mut self, key: KeyEvent, can_open: bool) -> bool {
        if !self.open {
            if can_open && key.code == KeyCode::Char('u') && key.modifiers.is_empty() {
                self.open = true;
                self.scroll = 0;
                self.reload();
                self.ask(false);
                return true;
            }
            return false;
        }
        match key.code {
            KeyCode::Char('?') => return false,
            KeyCode::Char('u') | KeyCode::Esc => self.open = false,
            KeyCode::Char('r') => self.ask(false),
            KeyCode::Enter if self.needs_keychain() => self.ask(true),
            KeyCode::Char('j') | KeyCode::Down => {
                self.scroll = (self.scroll + 1).min(self.lines.saturating_sub(self.height))
            }
            KeyCode::Char('k') | KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
            _ => {}
        }
        true
    }

    fn needs_keychain(&self) -> bool {
        self.snapshot.as_ref().is_some_and(|s| {
            s.providers
                .iter()
                .any(|p| p.provider == ProviderId::Claude && p.status == Status::NeedsKeychain)
        })
    }

    pub fn refreshing(&self) -> bool {
        self.request.is_some()
    }

    /// Starts a request unless this sidebar already has one in flight. Allowing the Keychain
    /// waits for the daemon's current fetch and then runs its own.
    fn ask(&mut self, allow_keychain: bool) {
        if self.request.is_some() && !allow_keychain {
            return;
        }
        let (send, receive) = channel();
        let command = if allow_keychain {
            "quota-allow-keychain"
        } else {
            "quota-refresh"
        };
        std::thread::spawn(move || {
            let _ = send.send(crate::lifecycle::ask(command, super::REQUEST_TIMEOUT));
        });
        self.request = Some(receive);
        self.requested_at = Some(Instant::now());
    }

    fn reload(&mut self) {
        match super::read() {
            Ok(snapshot) => self.snapshot = snapshot,
            Err(error) => self.notice = Some(format!("{error:#}")),
        }
    }

    /// Called on every pass of the sidebar loop: picks up a finished request, refreshes every
    /// five minutes, and rereads the snapshot another reader may have refreshed.
    pub fn tick(&mut self) {
        if !self.open {
            return;
        }
        if let Some(request) = &self.request {
            match request.try_recv() {
                Ok(result) => {
                    self.request = None;
                    self.notice = result.err().map(|e| format!("refresh failed: {e:#}"));
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    self.request = None;
                    self.notice = Some("refresh failed: request thread ended".into());
                }
            }
        }
        if self.request.is_none() && self.requested_at.is_none_or(|at| at.elapsed() >= EVERY) {
            self.ask(false);
        }
        self.reload();
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect) {
        self.draw_at(f, area, Utc::now());
    }

    fn draw_at(&mut self, f: &mut Frame, area: Rect, now: super::Time) {
        let [body, footer] =
            Layout::vertical([Constraint::Min(3), Constraint::Length(2)]).areas(area);
        let block = panel(Line::from(format!(
            " {} ",
            render::title(self.snapshot.as_ref(), self.refreshing())
        )));
        let inner = block.inner(body);
        f.render_widget(block, body);
        // One column of air on each side inside the frame.
        let cards = Rect {
            x: inner.x + 1,
            width: inner.width.saturating_sub(2),
            ..inner
        };
        let lines = match &self.snapshot {
            Some(snapshot) => render::cards(snapshot, now, usize::from(cards.width).max(1), true),
            None => vec![Line::styled(" no quota read yet", OVERLAY)],
        };
        self.lines = lines.len();
        self.height = usize::from(inner.height);
        self.scroll = self.scroll.min(self.lines.saturating_sub(self.height));
        f.render_widget(Paragraph::new(lines).scroll((self.scroll as u16, 0)), cards);
        let mut keys = Vec::new();
        if self.needs_keychain() {
            keys.push(("⏎", "allow"));
        }
        keys.extend([("r", "refresh"), ("u", "back")]);
        if self.lines > self.height {
            keys.push(("j k", "scroll"));
        }
        let status = match &self.notice {
            Some(notice) => Line::styled(
                truncate(&format!(" {notice}"), usize::from(area.width)),
                colour(State::Blocked),
            ),
            None => Line::default(),
        };
        f.render_widget(Paragraph::new(vec![hints(&keys), status]), footer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn screen(view: &mut View, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let now = crate::quota::tests::at(render::tests::NOW);
        terminal.draw(|f| view.draw_at(f, f.area(), now)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn draws_cards_in_a_panel_with_the_view_keys() {
        let mut view = View {
            open: true,
            snapshot: Some(render::tests::sample()),
            ..View::default()
        };
        let rows = screen(&mut view, 38, 40);
        assert!(rows[0].contains(" quota · "), "{}", rows[0]);
        let all = rows.join("\n");
        assert!(all.contains("● claude"), "{all}");
        assert!(
            all.contains("◐ codex") && all.contains("claude/gpt"),
            "{all}"
        );
        assert!(all.contains("○ grok"));
        assert!(rows[38].contains("r refresh") && rows[38].contains("u back"));
        assert!(!rows[38].contains("allow"));
    }

    #[test]
    fn keys_close_scroll_and_leave_help_alone() {
        let mut view = View {
            open: true,
            snapshot: Some(render::tests::sample()),
            ..View::default()
        };
        screen(&mut view, 38, 12);
        assert!(view.lines > view.height);
        assert!(view.key(KeyEvent::from(KeyCode::Char('j')), true));
        assert_eq!(view.scroll, 1);
        assert!(view.key(KeyEvent::from(KeyCode::Char('k')), true));
        assert_eq!(view.scroll, 0);
        assert!(
            view.key(KeyEvent::from(KeyCode::Char('x')), true),
            "the list never sees keys"
        );
        assert!(
            !view.key(KeyEvent::from(KeyCode::Char('?')), true),
            "help opens over it"
        );
        assert!(view.key(KeyEvent::from(KeyCode::Esc), true));
        assert!(!view.open);
        assert!(
            !view.key(KeyEvent::from(KeyCode::Char('u')), false),
            "not while typing"
        );
        assert!(!view.open);
    }
}
