//! tmux window layouts (`#{window_layout}`), so a recorded layout can leave out the sidebar.
//!
//! A layout is `checksum,` and a cell: `WxH,X,Y` followed by `,ID` for a pane, `{cells}` for
//! panes side by side or `[cells]` for panes one above the other.

use anyhow::{Context, Result, bail, ensure};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Cell {
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    node: Node,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Node {
    /// The pane id's number: `%12` is 12.
    Pane(u32),
    /// Side by side: `{...}`.
    Row(Vec<Cell>),
    /// One above the other: `[...]`.
    Column(Vec<Cell>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Axis {
    Horizontal,
    Vertical,
}

/// The layout without the given panes, whose space goes to their neighbours as when tmux closes
/// a pane. Every other pane keeps its place and order.
pub fn without(layout: &str, panes: &[&str]) -> Result<String> {
    let mut cell = parse(layout)?;
    for pane in panes {
        let id: u32 = pane
            .strip_prefix('%')
            .and_then(|n| n.parse().ok())
            .with_context(|| format!("invalid pane id {pane:?}"))?;
        ensure!(
            cell.node != Node::Pane(id),
            "cannot remove the only pane {pane} from layout {layout:?}"
        );
        ensure!(
            remove(&mut cell, id),
            "pane {pane} is not in layout {layout:?}"
        );
    }
    let body = cell.to_string();
    Ok(format!("{:04x},{body}", checksum(&body)))
}

/// The number of panes a layout holds.
#[cfg(test)]
pub fn panes(layout: &str) -> Result<usize> {
    fn count(cell: &Cell) -> usize {
        match &cell.node {
            Node::Pane(_) => 1,
            Node::Row(children) | Node::Column(children) => children.iter().map(count).sum(),
        }
    }
    Ok(count(&parse(layout)?))
}

fn checksum(body: &str) -> u16 {
    body.bytes().fold(0u16, |sum, byte| {
        ((sum >> 1) | ((sum & 1) << 15)).wrapping_add(u16::from(byte))
    })
}

fn parse(layout: &str) -> Result<Cell> {
    let (sum, body) = layout
        .split_once(',')
        .with_context(|| format!("layout without checksum: {layout:?}"))?;
    let sum = u16::from_str_radix(sum, 16).with_context(|| format!("layout checksum {sum:?}"))?;
    ensure!(
        checksum(body) == sum,
        "layout checksum does not match: {layout:?}"
    );
    let mut parser = Parser {
        bytes: body.as_bytes(),
        at: 0,
    };
    let cell = parser
        .cell()
        .with_context(|| format!("parsing layout {layout:?}"))?;
    ensure!(
        parser.at == body.len(),
        "trailing text in layout {layout:?}"
    );
    Ok(cell)
}

struct Parser<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Parser<'_> {
    fn number(&mut self) -> Result<u32> {
        let start = self.at;
        while self.bytes.get(self.at).is_some_and(u8::is_ascii_digit) {
            self.at += 1;
        }
        std::str::from_utf8(&self.bytes[start..self.at])?
            .parse()
            .with_context(|| format!("number expected at {start}"))
    }

    fn expect(&mut self, byte: u8) -> Result<()> {
        if self.bytes.get(self.at) != Some(&byte) {
            bail!("{:?} expected at {}", byte as char, self.at);
        }
        self.at += 1;
        Ok(())
    }

    fn cell(&mut self) -> Result<Cell> {
        let width = self.number()?;
        self.expect(b'x')?;
        let height = self.number()?;
        self.expect(b',')?;
        let x = self.number()?;
        self.expect(b',')?;
        let y = self.number()?;
        let node = match self.bytes.get(self.at) {
            Some(b',') => {
                self.at += 1;
                Node::Pane(self.number()?)
            }
            Some(&open @ (b'{' | b'[')) => {
                self.at += 1;
                let close = if open == b'{' { b'}' } else { b']' };
                let mut children = vec![self.cell()?];
                while self.bytes.get(self.at) == Some(&b',') {
                    self.at += 1;
                    children.push(self.cell()?);
                }
                self.expect(close)?;
                if open == b'{' {
                    Node::Row(children)
                } else {
                    Node::Column(children)
                }
            }
            _ => bail!("pane id or split expected at {}", self.at),
        };
        Ok(Cell {
            width,
            height,
            x,
            y,
            node,
        })
    }
}

impl std::fmt::Display for Cell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}x{},{},{}", self.width, self.height, self.x, self.y)?;
        let (open, close, children) = match &self.node {
            Node::Pane(id) => return write!(f, ",{id}"),
            Node::Row(children) => ('{', '}', children),
            Node::Column(children) => ('[', ']', children),
        };
        write!(f, "{open}")?;
        for (i, child) in children.iter().enumerate() {
            if i > 0 {
                write!(f, ",")?;
            }
            write!(f, "{child}")?;
        }
        write!(f, "{close}")
    }
}

/// Remove a pane below this cell. A split left with one child becomes that child.
fn remove(cell: &mut Cell, id: u32) -> bool {
    let (axis, children) = match &mut cell.node {
        Node::Pane(_) => return false,
        Node::Row(children) => (Axis::Horizontal, children),
        Node::Column(children) => (Axis::Vertical, children),
    };
    let Some(i) = children
        .iter()
        .position(|child| child.node == Node::Pane(id))
    else {
        return children.iter_mut().any(|child| remove(child, id));
    };
    let gone = children.remove(i);
    // Its space and the border beside it.
    let space = match axis {
        Axis::Horizontal => gone.width,
        Axis::Vertical => gone.height,
    } + 1;
    if i < children.len() {
        grow(&mut children[i], axis, space, true);
    } else {
        grow(&mut children[i - 1], axis, space, false);
    }
    if children.len() == 1 {
        let only = children.pop().expect("one child");
        cell.node = only.node;
    }
    true
}

/// Widen or heighten a cell by its leading (left or top) or trailing edge.
fn grow(cell: &mut Cell, axis: Axis, by: u32, leading: bool) {
    match axis {
        Axis::Horizontal => {
            cell.width += by;
            if leading {
                cell.x -= by;
            }
        }
        Axis::Vertical => {
            cell.height += by;
            if leading {
                cell.y -= by;
            }
        }
    }
    match (&mut cell.node, axis) {
        (Node::Pane(_), _) => {}
        // Along the split only the child on that edge grows; across it, all of them do.
        (Node::Row(children), Axis::Horizontal) | (Node::Column(children), Axis::Vertical) => {
            let edge = if leading {
                children.first_mut()
            } else {
                children.last_mut()
            };
            grow(edge.expect("a split has children"), axis, by, leading);
        }
        (Node::Row(children) | Node::Column(children), _) => {
            for child in children {
                grow(child, axis, by, leading);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Written by tmux 3.7c: a 38-column sidebar (%3), then %0, then %1 above %2.
    const WITH_SIDEBAR: &str =
        "1846,120x40,0,0{38x40,0,0,3,40x40,39,0,0,40x40,80,0[40x20,80,0,1,40x19,80,21,2]}";

    #[test]
    fn tmux_checksums_are_reproduced() {
        assert_eq!(parse(WITH_SIDEBAR).unwrap().to_string(), &WITH_SIDEBAR[5..]);
        assert!(parse(&WITH_SIDEBAR.replacen("1846", "1847", 1)).is_err());
        assert_eq!(panes(WITH_SIDEBAR).unwrap(), 4);
        // A one-pane window.
        assert_eq!(panes("b25d,80x24,0,0,0").unwrap(), 1);
    }

    #[test]
    fn a_sidebar_gives_its_columns_to_the_pane_beside_it() {
        let stripped = without(WITH_SIDEBAR, &["%3"]).unwrap();
        let body = "120x40,0,0{79x40,0,0,0,40x40,80,0[40x20,80,0,1,40x19,80,21,2]}";
        assert_eq!(stripped, format!("{:04x},{body}", checksum(body)));
        assert_eq!(panes(&stripped).unwrap(), 3);
    }

    #[test]
    fn a_split_left_with_one_pane_becomes_that_pane_across_every_level() {
        // Sidebar and one pane: what is left is the pane at full size.
        let body = "120x40,0,0{38x40,0,0,5,81x40,39,0,6}";
        let layout = format!("{:04x},{body}", checksum(body));
        let stripped = without(&layout, &["%5"]).unwrap();
        assert_eq!(&stripped[5..], "120x40,0,0,6");
        // Sidebar beside a column: every pane of the column widens to the left.
        let body = "120x40,0,0{38x40,0,0,5,81x40,39,0[81x20,39,0,6,81x19,39,21{40x19,39,21,7,40x19,80,21,8}]}";
        let layout = format!("{:04x},{body}", checksum(body));
        let stripped = without(&layout, &["%5"]).unwrap();
        assert_eq!(
            &stripped[5..],
            "120x40,0,0[120x20,0,0,6,120x19,0,21{79x19,0,21,7,40x19,80,21,8}]"
        );
        // The last pane of a column gives its rows to the one above.
        let body = "80x24,0,0[80x12,0,0,1,80x11,0,13,2]";
        let layout = format!("{:04x},{body}", checksum(body));
        assert_eq!(&without(&layout, &["%2"]).unwrap()[5..], "80x24,0,0,1");
    }

    #[test]
    fn unknown_or_only_panes_fail() {
        assert!(without(WITH_SIDEBAR, &["%9"]).is_err());
        assert!(without("b25d,80x24,0,0,0", &["%0"]).is_err());
        assert!(without(WITH_SIDEBAR, &["3"]).is_err());
        assert!(parse("garbage").is_err());
    }
}
