//! `p`: the agent list as a tree of who spawned whom, shared by the sidebar and the picker. Rows
//! stay one per agent, in depth-first order, so selection and every action work as in the list.
//! Each family keeps the place of its root; children follow their parent in list order.
use crate::index::{Agent, State};
use crate::ui::{SURFACE2, colour};
use ratatui::style::Style;
use ratatui::text::Span;
use std::collections::HashMap;

/// Deeper levels share the last indent, so a name always has room.
const MAX_DEPTH: usize = 4;

/// Where a row sits in the tree.
#[derive(Debug, PartialEq)]
pub(crate) struct Shape {
    /// 0 for a root.
    pub depth: usize,
    /// For each ancestor below the root, whether a later sibling of it still follows: its rail
    /// continues past this row.
    pub rails: Vec<bool>,
    /// The last child of its parent.
    pub last: bool,
    /// It has children in the list.
    pub parent_of: bool,
    /// A root that has a parent the list does not show.
    pub away: Option<Away>,
}

#[derive(Debug, PartialEq)]
pub(crate) enum Away {
    /// The parent has exited.
    Ended,
    /// The parent is running but hidden by a filter: its name.
    Hidden(String),
}

/// `agents` in tree order with each row's shape. `all` names parents a filter hides.
pub(crate) fn build<'a>(agents: &'a [Agent], all: &[Agent]) -> Vec<(&'a Agent, Shape)> {
    let listed = |pane: &str| agents.iter().any(|a| a.pane == pane);
    let mut children: HashMap<&str, Vec<&Agent>> = HashMap::new();
    let mut roots = Vec::new();
    for agent in agents {
        match agent.parent.as_deref() {
            Some(parent) if listed(parent) => children.entry(parent).or_default().push(agent),
            _ => roots.push(agent),
        }
    }
    let mut rows = Vec::new();
    for root in roots {
        let away = match &root.parent {
            Some(parent) => all
                .iter()
                .find(|a| &a.pane == parent)
                .map(|a| Away::Hidden(a.name.clone())),
            None if root.parent_ended => Some(Away::Ended),
            None => None,
        };
        walk(root, 0, Vec::new(), true, away, &children, &mut rows);
    }
    rows
}

fn walk<'a>(
    agent: &'a Agent,
    depth: usize,
    rails: Vec<bool>,
    last: bool,
    away: Option<Away>,
    children: &HashMap<&str, Vec<&'a Agent>>,
    rows: &mut Vec<(&'a Agent, Shape)>,
) {
    let kids = children
        .get(agent.pane.as_str())
        .map_or(&[][..], Vec::as_slice);
    rows.push((
        agent,
        Shape {
            depth,
            rails: rails.clone(),
            last,
            parent_of: !kids.is_empty(),
            away,
        },
    ));
    for (i, child) in kids.iter().enumerate() {
        let mut rails = rails.clone();
        // A root has no rail of its own: its children hang from its box.
        if depth > 0 {
            rails.push(!last);
        }
        walk(
            child,
            depth + 1,
            rails,
            i + 1 == kids.len(),
            None,
            children,
            rows,
        );
    }
}

/// `agents` reordered into the tree: what the list selects and acts on.
pub(crate) fn order(agents: &[Agent], all: &[Agent]) -> Vec<Agent> {
    build(agents, all)
        .into_iter()
        .map(|(agent, _)| agent.clone())
        .collect()
}

/// The line from a child to its parent: dashed while the child works, red while it is blocked.
pub(crate) fn edge(state: State) -> (&'static str, Style) {
    match state {
        State::Working => ("╌", Style::default().fg(colour(State::Working))),
        State::Blocked => ("─", Style::default().fg(colour(State::Blocked))),
        State::Done | State::Idle => ("─", Style::default().fg(SURFACE2)),
    }
}

/// The connectors before a row's state icon: ancestor rails, then its own branch and edge.
/// Empty for a root. Each level is two columns wide.
pub(crate) fn connectors(shape: &Shape, state: State) -> Vec<Span<'static>> {
    if shape.depth == 0 {
        return Vec::new();
    }
    let rail = Style::default().fg(SURFACE2);
    let mut spans = Vec::new();
    let hidden = shape.rails.len().saturating_sub(MAX_DEPTH - 1);
    if hidden > 0 {
        spans.push(Span::styled("┆ ", rail));
    }
    for &continues in shape.rails.iter().skip(hidden + usize::from(hidden > 0)) {
        spans.push(Span::styled(if continues { "│ " } else { "  " }, rail));
    }
    let (line, style) = edge(state);
    spans.push(Span::styled(if shape.last { "╰" } else { "├" }, style));
    spans.push(Span::styled(line, style));
    spans
}

/// What follows a root's name when its parent is not in the list.
pub(crate) fn away(shape: &Shape) -> Option<String> {
    match shape.away.as_ref()? {
        Away::Ended => Some(" ↑ ended".into()),
        Away::Hidden(name) => Some(format!(" ↑ {name}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::links::tests::agent;

    fn child(pane: &str, parent: &str) -> Agent {
        let mut agent = agent(pane);
        agent.parent = Some(parent.into());
        agent
    }

    fn text(spans: &[Span]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn families_keep_their_roots_place_and_children_follow_in_list_order() {
        // List order: solo, a grandchild, the lead, its two children.
        let agents = [
            agent("%9"),
            child("%4", "%2"),
            agent("%1"),
            child("%2", "%1"),
            child("%3", "%1"),
        ];
        let rows = build(&agents, &agents);
        let panes: Vec<&str> = rows.iter().map(|(a, _)| a.pane.as_str()).collect();
        assert_eq!(panes, ["%9", "%1", "%2", "%4", "%3"]);
        let shape = |pane: &str| &rows.iter().find(|(a, _)| a.pane == pane).unwrap().1;
        assert!(!shape("%9").parent_of && shape("%1").parent_of);
        assert_eq!((shape("%2").depth, shape("%2").last), (1, false));
        assert_eq!(shape("%4").rails, [true]);
        assert!(shape("%3").last);
        // Tree order is stable: ordering it again changes nothing.
        let ordered = order(&agents, &agents);
        assert_eq!(
            order(&ordered, &ordered)
                .iter()
                .map(|a| &a.pane)
                .collect::<Vec<_>>(),
            panes
        );
    }

    #[test]
    fn connectors_draw_rails_branches_and_state_edges() {
        let agents = [
            agent("%1"),
            child("%2", "%1"),
            child("%3", "%2"),
            child("%4", "%1"),
        ];
        let rows = build(&agents, &agents);
        let drawn: Vec<String> = rows
            .iter()
            .map(|(a, shape)| text(&connectors(shape, a.state)))
            .collect();
        assert_eq!(drawn, ["", "├─", "│ ╰─", "╰─"]);
        let (line, _) = edge(State::Working);
        assert_eq!(line, "╌");
    }

    #[test]
    fn deep_trees_stop_indenting_and_mark_the_hidden_levels() {
        let mut agents = vec![agent("%0")];
        for depth in 1..=7 {
            agents.push(child(&format!("%{depth}"), &format!("%{}", depth - 1)));
        }
        let rows = build(&agents, &agents);
        let widths: Vec<usize> = rows
            .iter()
            .map(|(a, shape)| text(&connectors(shape, a.state)).chars().count())
            .collect();
        assert_eq!(widths, [0, 2, 4, 6, 8, 8, 8, 8]);
        assert!(text(&connectors(&rows[7].1, State::Idle)).starts_with('┆'));
    }

    #[test]
    fn a_root_says_when_its_parent_ended_or_is_filtered_out() {
        let mut orphan = agent("%5");
        orphan.parent_ended = true;
        let all = [agent("%1"), child("%2", "%1"), orphan.clone()];
        let listed = [child("%2", "%1"), orphan];
        let rows = build(&listed, &all);
        assert_eq!(away(&rows[0].1).as_deref(), Some(" ↑ 1"));
        assert_eq!(away(&rows[1].1).as_deref(), Some(" ↑ ended"));
    }
}
