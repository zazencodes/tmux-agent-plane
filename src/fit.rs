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
        let command = command(size, self.default_size, views, geometry);
        if command.is_empty() {
            return Vec::new();
        }
        let command: Vec<_> = command.iter().map(String::as_str).collect();
        match tmux::run(&command) {
            Ok(_) => {
                self.default_size = Some(size);
                Vec::new()
            }
            // A window can close after the snapshot. The next tick retries on a fresh one, so
            // only a second failure in a row is a fault.
            Err(_) if !failed => {
                self.failed = true;
                Vec::new()
            }
            Err(error) => vec![Issue::new("fit", error)],
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

/// One tmux command, so each window goes straight to its final layout and its panes get one
/// resize. `resize-window` pins the window to manual sizing; unsetting that hands it back to tmux.
/// `resize-pane` would unzoom a zoomed window, so those wait for tmux to size them on arrival.
fn command(
    size: Size,
    default_size: Option<Size>,
    views: &[WindowView],
    geometry: &Geometry,
) -> Vec<String> {
    let mut commands: Vec<Vec<String>> = Vec::new();
    if default_size != Some(size) {
        // New windows in sessions nobody is attached to start at this size, and agents start there.
        commands.push(args(&[
            "set-option",
            "-g",
            "default-size",
            &format!("{}x{}", size.width, size.height),
        ]));
    }
    for view in views.iter().filter(|view| !view.viewed) {
        let stale = geometry.windows.iter().any(|window| {
            window.id == view.id && window.size != size && window.sizing == LATEST && !window.zoomed
        });
        if !stale {
            continue;
        }
        commands.push(args(&[
            "resize-window",
            "-t",
            &view.id,
            "-x",
            &size.width.to_string(),
            "-y",
            &size.height.to_string(),
        ]));
        if let Some(pane) = &view.sidebar {
            commands.push(args(&["resize-pane", "-t", pane, "-x", sidebar::WIDTH]));
        }
        commands.push(args(&["set-option", "-wu", "-t", &view.id, "window-size"]));
    }
    commands.join(&";".to_string())
}

fn args(args: &[&str]) -> Vec<String> {
    args.iter().map(ToString::to_string).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tmux::{ClientSize, WindowSize};

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
        let command = command(
            SCREEN,
            Some(SCREEN),
            &[view("@1", false, Some("%9"))],
            &geometry(vec![window("@1", 80)]),
        );
        assert_eq!(
            command.join(" "),
            format!(
                "resize-window -t @1 -x 200 -y 50 ; resize-pane -t %9 -x {} ; \
                 set-option -wu -t @1 window-size",
                sidebar::WIDTH
            )
        );
    }

    #[test]
    fn nothing_runs_once_every_window_fits() {
        let mut fitted = window("@1", 200);
        fitted.size.height = 50;
        assert!(
            command(
                SCREEN,
                Some(SCREEN),
                &[view("@1", false, None)],
                &geometry(vec![fitted]),
            )
            .is_empty()
        );
    }

    #[test]
    fn windows_on_screen_zoomed_or_sized_by_the_user_are_left_alone() {
        let mut zoomed = window("@2", 80);
        zoomed.zoomed = true;
        let mut manual = window("@3", 80);
        manual.sizing = "manual".into();
        assert!(
            command(
                SCREEN,
                Some(SCREEN),
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
    fn a_new_size_becomes_the_default_for_new_windows_once() {
        let command = command(
            SCREEN,
            Some(Size {
                width: 1,
                height: 1,
            }),
            &[],
            &geometry(vec![]),
        );
        assert_eq!(command.join(" "), "set-option -g default-size 200x50");
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
