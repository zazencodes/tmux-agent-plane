//! The sidebar's quota section: `u` expands or collapses details. It reads quota.json
//! and asks the daemon for a fresh read when stale, every five minutes while shown, and on `r`;
//! the request waits on its own thread, so the sidebar stays responsive.

use super::{ProviderId, Snapshot, Status, render};
use crate::index::State;
use crate::ui::{OVERLAY, colour, hints, section_heading, truncate};
use anyhow::Result;
use chrono::Utc;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};

const EVERY: Duration = Duration::from_secs(300);

/// What a sidebar knows of quota.json, and its own refresh request. The full view and the
/// panel share one.
#[derive(Default)]
pub struct Feed {
    snapshot: Option<Snapshot>,
    /// This sidebar's request, while the daemon works on it.
    request: Option<Receiver<Result<()>>>,
    requested_at: Option<Instant>,
    /// Why the last request or read failed.
    notice: Option<String>,
}

impl Feed {
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

    /// Picks up a finished request, rereads the snapshot another reader may have refreshed, and
    /// asks again once both this sidebar's last request and the snapshot are five minutes old,
    /// so many sidebars share one read.
    fn tick(&mut self, now: super::Time) {
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
        self.reload();
        if self.request.is_none() && due(self.requested_at, self.snapshot.as_ref(), now) {
            self.ask(false);
        }
    }
}

fn due(requested_at: Option<Instant>, snapshot: Option<&Snapshot>, now: super::Time) -> bool {
    requested_at.is_none_or(|at| at.elapsed() >= EVERY)
        && snapshot.is_none_or(|s| (now - s.updated_at).to_std().is_ok_and(|age| age >= EVERY))
}

#[derive(Default)]
pub struct View {
    /// `u`: expanded details above the bottom of the sidebar.
    pub open: bool,
    /// `u`: the panel below the graph and tasks; starts from the configuration.
    pub pinned: bool,
    feed: Feed,
    scroll: usize,
    /// Lines that fit, from the last draw.
    height: usize,
    lines: usize,
}

impl View {
    /// `u` expands or collapses quota. Browsing stays live while compact quota shows.
    /// While details are open, every key but `?` is the view's; Esc returns to the panel.
    pub fn key(&mut self, key: KeyEvent, can_open: bool) -> bool {
        let key = crate::ui::escape_key(key);
        if can_open && key.code == KeyCode::Char('u') && key.modifiers.is_empty() {
            if self.open {
                self.open = false;
            } else {
                self.pinned = true;
                self.open = true;
                self.scroll = 0;
            }
            return true;
        }
        if !self.open {
            return false;
        }
        match key.code {
            KeyCode::Char('?') => return false,
            KeyCode::Esc => self.open = false,
            KeyCode::Char('r') => self.feed.ask(false),
            KeyCode::Enter if self.feed.needs_keychain() => self.feed.ask(true),
            KeyCode::Char('j') | KeyCode::Down => {
                self.scroll = (self.scroll + 1).min(self.lines.saturating_sub(self.height))
            }
            KeyCode::Char('k') | KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
            _ => {}
        }
        true
    }

    /// Called on every pass of the sidebar loop while the view or the panel shows.
    pub fn tick(&mut self) {
        if self.open || self.pinned {
            self.feed.tick(Utc::now());
        }
    }

    /// One row of spacing, a heading, then one row per active provider.
    pub fn panel_height(&self) -> u16 {
        let rows = self
            .feed
            .snapshot
            .as_ref()
            .map_or(1, |s| render::summary(s, Utc::now(), 1).len().max(1));
        rows as u16 + 2
    }

    /// The pinned panel: one line per provider.
    pub fn draw_panel(&self, f: &mut Frame, area: Rect) {
        self.draw_panel_at(f, area, Utc::now());
    }

    fn draw_panel_at(&self, f: &mut Frame, area: Rect, now: super::Time) {
        let inner = area.inner(Margin::new(1, 0));
        let [_, heading, rows] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .areas(inner);
        let title = if self.feed.refreshing() {
            "quota · refreshing…"
        } else {
            "quota"
        };
        f.render_widget(
            Paragraph::new(section_heading(title, usize::from(inner.width))),
            heading,
        );
        let width = usize::from(inner.width).max(1);
        let lines = match (&self.feed.notice, &self.feed.snapshot) {
            (Some(notice), _) => vec![Line::styled(
                truncate(notice, width),
                colour(State::Blocked),
            )],
            (None, Some(snapshot)) => render::summary(snapshot, now, width),
            (None, None) => vec![Line::styled("no quota read yet", OVERLAY)],
        };
        let lines = if lines.is_empty() {
            vec![Line::styled("no plan set up", OVERLAY)]
        } else {
            lines
        };
        f.render_widget(Paragraph::new(lines), rows);
    }

    pub fn draw(&mut self, f: &mut Frame, area: Rect) {
        self.draw_at(f, area, Utc::now());
    }

    fn draw_at(&mut self, f: &mut Frame, area: Rect, now: super::Time) {
        let [body, footer] =
            Layout::vertical([Constraint::Min(3), Constraint::Length(2)]).areas(area);
        let inner = body.inner(Margin::new(1, 0));
        let [_, heading, cards] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .areas(inner);
        f.render_widget(
            Paragraph::new(section_heading(
                &render::title(self.feed.snapshot.as_ref(), self.feed.refreshing()),
                usize::from(inner.width),
            )),
            heading,
        );
        let lines = match &self.feed.snapshot {
            Some(snapshot) => render::cards(snapshot, now, usize::from(cards.width).max(1), true),
            None => vec![Line::styled(" no quota read yet", OVERLAY)],
        };
        self.lines = lines.len();
        self.height = usize::from(cards.height);
        self.scroll = self.scroll.min(self.lines.saturating_sub(self.height));
        f.render_widget(Paragraph::new(lines).scroll((self.scroll as u16, 0)), cards);
        let mut keys = Vec::new();
        if self.feed.needs_keychain() {
            keys.push(("⏎", "allow"));
        }
        keys.extend([("r", "refresh"), ("esc", "back"), ("u", "collapse")]);
        if self.lines > self.height {
            keys.push(("j k", "scroll"));
        }
        let status = match &self.feed.notice {
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
            feed: Feed {
                snapshot: Some(render::tests::sample()),
                ..Feed::default()
            },
            ..View::default()
        };
        let rows = screen(&mut view, 38, 40);
        assert!(rows[1].contains(" quota · "), "{}", rows[1]);
        let all = rows.join("\n");
        assert!(all.contains("● claude"), "{all}");
        assert!(
            all.contains("◐ codex") && all.contains("claude/gpt"),
            "{all}"
        );
        assert!(!all.contains("○ grok"));
        assert!(rows[38].contains("r refresh") && rows[38].contains("u collapse"));
        assert!(!rows[38].contains("allow"));
    }

    #[test]
    fn keys_close_scroll_and_leave_help_alone() {
        let mut view = View {
            open: true,
            feed: Feed {
                snapshot: Some(render::tests::sample()),
                ..Feed::default()
            },
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

    #[test]
    fn u_pins_quota_until_toggled_and_keeps_browsing_live() {
        let mut view = View::default();
        let key = |code| KeyEvent::from(code);
        assert!(!view.key(key(KeyCode::Char('u')), false));
        assert!(!view.pinned);
        assert!(!view.key(key(KeyCode::Char('i')), true));
        assert!(view.key(key(KeyCode::Char('u')), true));
        assert!(view.pinned && view.open);
        assert!(view.key(key(KeyCode::Char('u')), true));
        assert!(view.pinned && !view.open);
        for code in [
            KeyCode::Char('j'),
            KeyCode::Char('r'),
            KeyCode::Esc,
            KeyCode::Enter,
        ] {
            assert!(!view.key(key(code), true));
            assert!(view.pinned && !view.open);
        }
        assert!(!view.key(key(KeyCode::Char('i')), true));
        assert!(view.key(key(KeyCode::Char('u')), true));
        assert!(view.pinned && view.open);
        assert!(view.key(key(KeyCode::Esc), true));
        assert!(view.pinned && !view.open);
        assert!(!view.key(key(KeyCode::Char('i')), true));
        assert!(view.key(key(KeyCode::Char('u')), true));
        assert!(view.key(key(KeyCode::Char('u')), true));
        assert!(view.pinned && !view.open);
    }

    #[test]
    fn compact_quota_matches_the_tasks_section() {
        let view = View {
            pinned: true,
            feed: Feed {
                snapshot: Some(render::tests::sample()),
                ..Feed::default()
            },
            ..View::default()
        };
        let height = view.panel_height();
        assert_eq!(height, 5, "three active providers, spacing and heading");
        let mut terminal = Terminal::new(TestBackend::new(38, height)).unwrap();
        let now = crate::quota::tests::at(render::tests::NOW);
        terminal
            .draw(|f| view.draw_panel_at(f, f.area(), now))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let rows: Vec<String> = (0..height)
            .map(|y| (0..38).map(|x| buffer[(x, y)].symbol()).collect())
            .collect();
        assert!(rows[0].trim().is_empty());
        assert!(rows[1].starts_with(" ── quota ─"), "{}", rows[1]);
        assert!(rows[2].starts_with(" ● claude ━"), "{}", rows[2]);
        assert!(!rows.join("\n").contains("expired"));
        assert!(!rows.join("\n").contains("details"));
        assert!(!rows.join("\n").contains('│'));
    }
}
