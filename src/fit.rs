//! Keep windows nobody is looking at the size they will be shown at, so arriving at one resizes
//! nothing.
//!
//! Left to itself, tmux sizes a window only when a client arrives. It splits the change across the
//! panes, so an agent first redraws for a width that is wrong; the sidebar then takes its width
//! back, and tmux's 250 ms pane resize limit delays the real width. Fitting while the window is
//! hidden gives the agent one resize, to its final size, where nobody sees the redraw.
//!
//! Only tmux's default `window-size latest` is touched, and only while every client agrees on the
//! size: with several sizes on screen, nothing says which one a hidden window will be shown at.

use crate::diagnostics::Issue;
use crate::index::WindowView;
use crate::sidebar;
use crate::tmux::{self, Geometry, Size};
use anyhow::Result;

const LATEST: &str = "latest";

#[derive(Default)]
pub struct Fit {
    /// The size at the last tick. Acting only once it repeats means a terminal being dragged
    /// wakes every agent once, when it stops.
    seen: Option<Size>,
    /// The `default-size` this daemon has set.
    default_size: Option<Size>,
    failed: bool,
}

impl Fit {
    pub fn tick(&mut self, views: &[WindowView], geometry: &Geometry) -> Vec<Issue> {
        let failed = std::mem::take(&mut self.failed);
        let target = target(geometry);
        let settled = target.filter(|_| target == self.seen);
        self.seen = target;
        let Some(size) = settled else {
            return Vec::new();
        };
        let mut result = Ok(());
        if self.default_size != Some(size) {
            // New windows in sessions nobody is attached to start at this size, and agents start there.
            let value = format!("{}x{}", size.width, size.height);
            match tmux::run(&["set-option", "-g", "default-size", &value]) {
                Ok(_) => self.default_size = Some(size),
                Err(error) => result = Err(error),
            }
        }
        for command in window_commands(size, views, geometry) {
            let command: Vec<_> = command.iter().map(String::as_str).collect();
            if let Err(error) = tmux::run(&command) {
                result = result.and(Err(error));
            }
        }
        self.settle(failed, result)
    }

    /// A window can close after the snapshot. The next tick retries on a fresh one, so only a
    /// second failure in a row is a fault, and it stays one until a tick succeeds.
    fn settle(&mut self, failed: bool, result: Result<()>) -> Vec<Issue> {
        let Err(error) = result else {
            return Vec::new();
        };
        self.failed = true;
        if failed {
            vec![Issue::new("fit", error)]
        } else {
            Vec::new()
        }
    }
}

/// The size a hidden window will be shown at, read from the windows on screen so it already
/// accounts for each session's status lines.
fn target(geometry: &Geometry) -> Option<Size> {
    let first = geometry.clients.first()?;
    geometry
        .clients
        .iter()
        .all(|client| client.size == first.size && client.sizing == LATEST)
        .then_some(first.size)
}

/// One tmux command per window, so each goes straight to its final layout and its panes get one
/// resize, however many windows are stale. tmux refuses a single command that is too long.
/// `resize-window` pins the window to manual sizing; unsetting that hands it back to tmux.
/// `resize-pane` would unzoom a zoomed window, so those wait for tmux to size them on arrival.
fn window_commands(size: Size, views: &[WindowView], geometry: &Geometry) -> Vec<Vec<String>> {
    views
        .iter()
        .filter(|view| !view.viewed)
        .filter(|view| {
            geometry.windows.iter().any(|window| {
                window.id == view.id
                    && window.size != size
                    && window.sizing == LATEST
                    && !window.zoomed
            })
        })
        .map(|view| {
            let mut steps = vec![args(&[
                "resize-window",
                "-t",
                &view.id,
                "-x",
                &size.width.to_string(),
                "-y",
                &size.height.to_string(),
            ])];
            if let Some(pane) = &view.sidebar {
                steps.push(args(&["resize-pane", "-t", pane, "-x", sidebar::WIDTH]));
            }
            steps.push(args(&["set-option", "-wu", "-t", &view.id, "window-size"]));
            steps.join(&";".to_string())
        })
        .collect()
}

fn args(args: &[&str]) -> Vec<String> {
    args.iter().map(ToString::to_string).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tmux::{ClientSize, WindowSize};
    use anyhow::anyhow;

    const SCREEN: Size = Size {
        width: 200,
        height: 50,
    };

    fn client(width: u32) -> ClientSize {
        ClientSize {
            size: Size { width, height: 50 },
            sizing: LATEST.into(),
        }
    }

    fn window(id: &str, width: u32) -> WindowSize {
        WindowSize {
            id: id.into(),
            size: Size { width, height: 24 },
            zoomed: false,
            sizing: LATEST.into(),
        }
    }

    fn view(id: &str, viewed: bool, sidebar: Option<&str>) -> WindowView {
        WindowView {
            id: id.into(),
            active_pane: "%0".into(),
            viewed,
            sidebar: sidebar.map(Into::into),
        }
    }

    fn geometry(windows: Vec<WindowSize>) -> Geometry {
        Geometry {
            clients: vec![client(200)],
            windows,
        }
    }

    #[test]
    fn a_hidden_window_and_its_sidebar_go_to_the_final_layout_in_one_command() {
        let commands = window_commands(
            SCREEN,
            &[view("@1", false, Some("%9"))],
            &geometry(vec![window("@1", 80)]),
        );
        assert_eq!(commands.len(), 1);
        assert_eq!(
            commands[0].join(" "),
            format!(
                "resize-window -t @1 -x 200 -y 50 ; resize-pane -t %9 -x {} ; \
                 set-option -wu -t @1 window-size",
                sidebar::WIDTH
            )
        );
    }

    #[test]
    fn each_window_gets_its_own_command() {
        let ids: Vec<String> = (0..100).map(|id| format!("@{id}")).collect();
        let views: Vec<_> = ids.iter().map(|id| view(id, false, Some("%9"))).collect();
        let windows = ids.iter().map(|id| window(id, 80)).collect();
        assert_eq!(
            window_commands(SCREEN, &views, &geometry(windows)).len(),
            100
        );
    }

    #[test]
    fn nothing_runs_once_every_window_fits() {
        let mut fitted = window("@1", 200);
        fitted.size.height = 50;
        assert!(
            window_commands(SCREEN, &[view("@1", false, None)], &geometry(vec![fitted])).is_empty()
        );
    }

    #[test]
    fn windows_on_screen_zoomed_or_sized_by_the_user_are_left_alone() {
        let mut zoomed = window("@2", 80);
        zoomed.zoomed = true;
        let mut manual = window("@3", 80);
        manual.sizing = "manual".into();
        assert!(
            window_commands(
                SCREEN,
                &[
                    view("@1", true, None),
                    view("@2", false, None),
                    view("@3", false, None),
                ],
                &geometry(vec![window("@1", 80), zoomed, manual]),
            )
            .is_empty()
        );
    }

    #[test]
    fn a_fault_stays_raised_until_a_tick_succeeds() {
        let mut fit = Fit::default();
        let mut tick = |result: Result<()>| {
            let failed = std::mem::take(&mut fit.failed);
            fit.settle(failed, result).len()
        };
        assert_eq!(tick(Err(anyhow!("window closed"))), 0);
        assert_eq!(tick(Err(anyhow!("still failing"))), 1);
        assert_eq!(tick(Err(anyhow!("still failing"))), 1);
        assert_eq!(tick(Ok(())), 0);
        assert_eq!(tick(Err(anyhow!("window closed"))), 0);
    }

    #[test]
    fn clients_must_agree_on_a_size() {
        let mut geometry = geometry(vec![]);
        assert_eq!(target(&geometry), Some(SCREEN));
        geometry.clients.push(client(120));
        assert_eq!(target(&geometry), None);
        geometry.clients = vec![];
        assert_eq!(target(&geometry), None);
        geometry.clients = vec![ClientSize {
            size: SCREEN,
            sizing: "manual".into(),
        }];
        assert_eq!(target(&geometry), None);
    }
}
