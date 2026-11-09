//! `p`: the selected agent's family, drawn as a diagram the sidebar and the picker share. The
//! parent sits in a box on the left and its children in boxes down the right, joined by lines
//! that are dashed while a child works and red while it is blocked. The selected agent is lit.
use crate::index::{Agent, State};
use crate::ui::{LAVENDER, OVERLAY, SUBTEXT, SURFACE2, TEXT, age, colour, icon, truncate};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;

/// Columns between the parent's box and its children's: a stub, the trunk and a stub.
const LINK: u16 = 3;
/// Narrowest box worth drawing: borders, icon and a few letters.
const MIN_BOX: u16 = 8;

pub(crate) struct Family<'a> {
    /// The left node's own parent, so a middle generation still shows where it came from.
    above: Option<Above<'a>>,
    left: &'a Agent,
    right: Vec<&'a Agent>,
    lit: &'a str,
}

enum Above<'a> {
    Parent(&'a Agent),
    Ended,
}

/// The family around `selected`: its children when it has any, otherwise its parent and
/// siblings, otherwise itself alone.
pub(crate) fn family<'a>(all: &'a [Agent], selected: &'a Agent) -> Family<'a> {
    let children = |pane: &str| -> Vec<&Agent> {
        all.iter()
            .filter(|a| a.parent.as_deref() == Some(pane))
            .collect()
    };
    let parent = |agent: &Agent| {
        agent
            .parent
            .as_deref()
            .and_then(|pane| all.iter().find(|a| a.pane == pane))
    };
    let own = children(&selected.pane);
    let (left, right) = match parent(selected) {
        _ if !own.is_empty() => (selected, own),
        Some(parent) => (parent, children(&parent.pane)),
        None => (selected, Vec::new()),
    };
    let above = match parent(left) {
        Some(parent) => Some(Above::Parent(parent)),
        None if left.parent_ended => Some(Above::Ended),
        None => None,
    };
    Family {
        above,
        left,
        right,
        lit: &selected.pane,
    }
}

impl Family<'_> {
    /// No parent, running or ended, and no children.
    pub(crate) fn alone(&self) -> bool {
        self.above.is_none() && self.right.is_empty()
    }

    /// Rows the whole drawing takes.
    pub(crate) fn height(&self) -> u16 {
        u16::from(self.above.is_some()) + 3 * (self.right.len().max(1) as u16)
    }

    pub(crate) fn render(&self, area: Rect, buf: &mut Buffer) {
        let (x, mut y) = (area.x, area.y);
        let bottom = area.bottom();
        if let Some(above) = &self.above
            && y < bottom
        {
            let text = match above {
                Above::Parent(parent) => format!("↑ {}", parent.name),
                Above::Ended => "↑ parent ended".into(),
            };
            buf.set_string(x, y, truncate(&text, area.width as usize), OVERLAY);
            y += 1;
        }
        if self.right.is_empty() {
            let width = need(self.left).min(area.width);
            self.node(
                buf,
                x,
                y,
                width,
                bottom,
                self.left,
                width >= need(self.left),
            );
            return;
        }
        let room = area.width.saturating_sub(LINK);
        let widest = self.right.iter().map(|a| need(a)).max().unwrap_or(0);
        // The parent's name first, with its age when that fits in half the room; the children
        // the rest.
        let left_width = if need(self.left) <= room / 2 {
            need(self.left)
        } else {
            need_name(self.left).min(room / 2).max(MIN_BOX.min(room))
        };
        let right_width = widest.min(room.saturating_sub(left_width));
        if right_width < MIN_BOX {
            // Too narrow for two columns: the parent's box alone.
            let ages = area.width >= need(self.left);
            self.node(buf, x, y, area.width, bottom, self.left, ages);
            return;
        }
        self.node(
            buf,
            x,
            y,
            left_width,
            bottom,
            self.left,
            left_width >= need(self.left),
        );
        // Ages on every child or none, so they line up.
        let ages = right_width >= widest;
        let trunk = x + left_width + 1;
        let child_x = x + left_width + LINK;
        let rail = Style::default().fg(SURFACE2);
        let rows = bottom.saturating_sub(y);
        // Children that do not fit leave a line saying how many more there are.
        let shown = if (rows / 3) as usize >= self.right.len() {
            self.right.len()
        } else {
            (rows.saturating_sub(1) / 3).max(1) as usize
        };
        set(buf, trunk - 1, y + 1, bottom, "─", rail);
        for (i, child) in self.right.iter().take(shown).enumerate() {
            let top = y + 3 * i as u16;
            let middle = top + 1;
            let more = i + 1 < self.right.len();
            let joint = match (i, more) {
                (0, true) => "┬",
                (0, false) => "─",
                (_, true) => "├",
                (_, false) => "╰",
            };
            set(buf, trunk, middle, bottom, joint, rail);
            if more {
                set(buf, trunk, middle + 1, bottom, "│", rail);
                set(buf, trunk, middle + 2, bottom, "│", rail);
            }
            let (line, style) = edge(child.state);
            set(buf, trunk + 1, middle, bottom, line, style);
            self.node(buf, child_x, top, right_width, bottom, child, ages);
        }
        let hidden = self.right.len() - shown;
        let last = y + 3 * shown as u16;
        if hidden > 0 && last < bottom {
            set(buf, trunk, last, bottom, "╰", rail);
            buf.set_string(child_x, last, format!("+{hidden} more"), OVERLAY);
        }
    }

    /// One agent's box: state icon and name, and its age when asked.
    #[allow(clippy::too_many_arguments)]
    fn node(
        &self,
        buf: &mut Buffer,
        x: u16,
        y: u16,
        width: u16,
        bottom: u16,
        agent: &Agent,
        ages: bool,
    ) {
        if width < 4 || y + 3 > bottom {
            return;
        }
        let lit = agent.pane == self.lit;
        let border = if lit {
            Style::default().fg(LAVENDER).bold()
        } else {
            Style::default().fg(SURFACE2)
        };
        let inner = (width - 2) as usize;
        buf.set_string(x, y, format!("╭{}╮", "─".repeat(inner)), border);
        buf.set_string(x, y + 2, format!("╰{}╯", "─".repeat(inner)), border);
        buf.set_string(x, y + 1, "│", border);
        buf.set_string(x + width - 1, y + 1, "│", border);
        let since = if ages {
            format!(" {} ", age(agent.state_since))
        } else {
            String::new()
        };
        // ` ● ` before the name; after it a space, or the age with its own spaces.
        let name_room = inner.saturating_sub(3 + since.len().max(1));
        let name = if lit {
            Style::default().fg(TEXT).bold()
        } else if agent.state == State::Idle {
            Style::default().fg(SUBTEXT)
        } else {
            Style::default().fg(TEXT)
        };
        buf.set_string(
            x + 1,
            y + 1,
            format!(" {}", icon(agent.state)),
            colour(agent.state),
        );
        buf.set_string(x + 4, y + 1, truncate(&agent.name, name_room), name);
        if !since.is_empty() {
            let at = x + width - 1 - since.len() as u16;
            buf.set_string(at, y + 1, since, OVERLAY);
        }
    }
}

/// Columns a box needs for its icon, name and age: borders, ` ● `, the name, ` 4m `.
fn need(agent: &Agent) -> u16 {
    need_name(agent) + age(agent.state_since).len() as u16 + 1
}

/// Columns a box needs for its icon and name: borders, ` ● `, the name and a space.
fn need_name(agent: &Agent) -> u16 {
    agent.name.chars().count() as u16 + 6
}

fn set(buf: &mut Buffer, x: u16, y: u16, bottom: u16, symbol: &str, style: Style) {
    if y < bottom && x < buf.area.right() {
        buf.set_string(x, y, symbol, style);
    }
}

/// The line from a parent to a child: dashed while the child works, red while it is blocked.
pub(crate) fn edge(state: State) -> (&'static str, Style) {
    match state {
        State::Working => ("╌", Style::default().fg(colour(State::Working))),
        State::Blocked => ("─", Style::default().fg(colour(State::Blocked))),
        State::Done | State::Idle => ("─", Style::default().fg(SURFACE2)),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::links::tests::agent;

    /// site-lead spawned hero-copy, api-review and docs-pass; api-review spawned api-tests.
    pub(crate) fn family_agents() -> Vec<Agent> {
        let now = crate::index::now_ms();
        let member = |pane: &str, name: &str, state, parent: Option<&str>| {
            let mut a = agent(pane);
            a.name = name.into();
            a.state = state;
            a.parent = parent.map(Into::into);
            a.state_since = now;
            a
        };
        let mut orphan = member("%6", "reviewer", State::Idle, None);
        orphan.parent_ended = true;
        vec![
            member("%1", "site-lead", State::Working, None),
            member("%2", "hero-copy", State::Working, Some("%1")),
            member("%3", "api-review", State::Done, Some("%1")),
            member("%4", "api-tests", State::Blocked, Some("%3")),
            member("%5", "docs-pass", State::Idle, Some("%1")),
            orphan,
        ]
    }

    pub(crate) fn drawn(all: &[Agent], pane: &str, width: u16, height: u16) -> Vec<String> {
        let selected = all.iter().find(|a| a.pane == pane).unwrap();
        let family = family(all, selected);
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        family.render(area, &mut buf);
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    fn lit(all: &[Agent], pane: &str, name: &str) -> bool {
        let selected = all.iter().find(|a| a.pane == pane).unwrap();
        let family = family(all, selected);
        let area = Rect::new(0, 0, 60, 16);
        let mut buf = Buffer::empty(area);
        family.render(area, &mut buf);
        let row = (0..16)
            .find(|&y| {
                (0..60)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    .contains(name)
            })
            .unwrap();
        let line: Vec<&str> = (0..60).map(|x| buf[(x, row)].symbol()).collect();
        let start = (0..60)
            .find(|&x| line[x..].concat().starts_with(name))
            .unwrap();
        // The box's own left border, just before its icon.
        let border = (0..start).rev().find(|&x| line[x] == "│").unwrap();
        buf[(border as u16, row)].fg == LAVENDER
    }

    #[test]
    fn a_parent_shows_its_children_and_is_lit() {
        let all = family_agents();
        let rows = drawn(&all, "%1", 36, 10);
        assert_eq!(
            rows,
            [
                "╭─────────────╮   ╭────────────────╮",
                "│ ● site-lead │─┬╌│ ● hero-copy    │",
                "╰─────────────╯ │ ╰────────────────╯",
                "                │ ╭────────────────╮",
                "                ├─│ ✓ api-review   │",
                "                │ ╰────────────────╯",
                "                │ ╭────────────────╮",
                "                ╰─│ ○ docs-pass    │",
                "                  ╰────────────────╯",
                "",
            ],
            "{rows:#?}"
        );
        assert!(lit(&all, "%1", "site-lead") && !lit(&all, "%1", "hero-copy"));
    }

    #[test]
    fn a_child_shows_its_parent_and_siblings_and_is_the_one_lit() {
        let all = family_agents();
        let rows = drawn(&all, "%5", 60, 10);
        assert!(
            rows[1].contains("site-lead") && rows[7].contains("docs-pass"),
            "{rows:#?}"
        );
        assert!(lit(&all, "%5", "docs-pass") && !lit(&all, "%5", "site-lead"));
    }

    #[test]
    fn a_middle_generation_names_its_own_parent_above() {
        let all = family_agents();
        let rows = drawn(&all, "%3", 60, 6);
        assert_eq!(rows[0], "↑ site-lead");
        assert!(
            rows[2].contains("api-review") && rows[2].contains("◆ api-tests"),
            "{rows:#?}"
        );
        // Its parent's dashed edge to a working child, and red to a blocked one.
        assert_eq!(edge(State::Working).0, "╌");
        assert_eq!(edge(State::Blocked).1.fg, Some(colour(State::Blocked)));
    }

    #[test]
    fn an_ended_parent_is_named_and_only_a_lone_agent_is_alone() {
        let all = family_agents();
        let rows = drawn(&all, "%6", 36, 6);
        assert_eq!(rows[0], "↑ parent ended");
        assert!(rows[2].contains("○ reviewer"));
        assert!(rows[4].is_empty());
        let selected = |pane: &str| all.iter().find(|a| a.pane == pane).unwrap();
        assert!(!family(&all, selected("%6")).alone());
        assert!(!family(&all, selected("%5")).alone());
        let mut solo = all[5].clone();
        solo.parent_ended = false;
        assert!(family(std::slice::from_ref(&solo), &solo).alone());
    }

    #[test]
    fn children_that_do_not_fit_are_counted_and_small_areas_never_panic() {
        let all = family_agents();
        let rows = drawn(&all, "%1", 36, 7);
        assert_eq!(rows[4], "                ├─│ ✓ api-review   │", "{rows:#?}");
        assert!(rows[6].contains("╰ +1 more"), "{rows:#?}");
        for (width, height) in [(0, 0), (1, 1), (8, 3), (12, 4), (20, 2), (38, 30)] {
            for pane in ["%1", "%3", "%5", "%6"] {
                drawn(&all, pane, width, height);
            }
        }
    }
}
