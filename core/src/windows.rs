//! How split views share the screen: a tree whose leaves are views and
//! whose nodes put their children side by side or one above another, in
//! equal parts.

/// A part of the screen, in cells.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl Rect {
    fn center(self) -> (i32, i32) {
        (
            i32::from(self.x) * 2 + i32::from(self.width),
            i32::from(self.y) * 2 + i32::from(self.height),
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Splits {
    Leaf(u32),
    Split {
        /// Children side by side; otherwise one above another.
        side_by_side: bool,
        children: Vec<Splits>,
    },
}

/// A line drawn between views.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Separator {
    /// A column between views side by side.
    Column { x: u16, y: u16, height: u16 },
    /// A row below `view`, which shows its name.
    Row {
        x: u16,
        y: u16,
        width: u16,
        view: u32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

impl Splits {
    /// The views, in order: left to right, top to bottom.
    pub fn leaves(&self) -> Vec<u32> {
        match self {
            Splits::Leaf(id) => vec![*id],
            Splits::Split { children, .. } => children.iter().flat_map(Splits::leaves).collect(),
        }
    }

    /// Puts view `new` next to view `at`, after it.
    pub fn split(&mut self, at: u32, new: u32, side_by_side: bool) {
        match self {
            Splits::Leaf(id) if *id == at => {
                *self = Splits::Split {
                    side_by_side,
                    children: vec![Splits::Leaf(at), Splits::Leaf(new)],
                };
            }
            Splits::Leaf(_) => {}
            Splits::Split {
                side_by_side: same,
                children,
            } => {
                // Beside a sibling going the same way, rather than nesting.
                if *same == side_by_side
                    && let Some(i) = children.iter().position(|c| *c == Splits::Leaf(at))
                {
                    children.insert(i + 1, Splits::Leaf(new));
                    return;
                }
                for child in children {
                    child.split(at, new, side_by_side);
                }
            }
        }
    }

    /// Takes view `id` out, merging a split left with one child into its
    /// parent. The last view stays.
    pub fn remove(&mut self, id: u32) {
        let Splits::Split { children, .. } = self else {
            return;
        };
        children.retain(|c| *c != Splits::Leaf(id));
        for child in children.iter_mut() {
            child.remove(id);
        }
        if children.len() == 1 {
            *self = children.pop().expect("one child");
        }
    }

    /// Where each view goes in `area`, and the separators between them.
    pub fn layout(&self, area: Rect) -> (Vec<(u32, Rect)>, Vec<Separator>) {
        let mut rects = Vec::new();
        let mut separators = Vec::new();
        self.layout_into(area, &mut rects, &mut separators);
        (rects, separators)
    }

    fn layout_into(&self, area: Rect, rects: &mut Vec<(u32, Rect)>, seps: &mut Vec<Separator>) {
        let (side_by_side, children) = match self {
            Splits::Leaf(id) => {
                rects.push((*id, area));
                return;
            }
            Splits::Split {
                side_by_side,
                children,
            } => (*side_by_side, children),
        };
        let n = children.len() as u16;
        let total = if side_by_side {
            area.width
        } else {
            area.height
        };
        // One cell between children for a separator.
        let space = total.saturating_sub(n - 1);
        let mut at = if side_by_side { area.x } else { area.y };
        for (i, child) in children.iter().enumerate() {
            let i = i as u16;
            // The last child takes what division leaves over.
            let size = if i + 1 == n {
                space - (space / n) * (n - 1)
            } else {
                space / n
            };
            let part = if side_by_side {
                Rect {
                    x: at,
                    width: size,
                    ..area
                }
            } else {
                Rect {
                    y: at,
                    height: size,
                    ..area
                }
            };
            child.layout_into(part, rects, seps);
            at += size;
            if i + 1 < n {
                seps.push(if side_by_side {
                    Separator::Column {
                        x: at,
                        y: area.y,
                        height: area.height,
                    }
                } else {
                    Separator::Row {
                        x: area.x,
                        y: at,
                        width: area.width,
                        view: child.leaves()[0],
                    }
                });
                at += 1;
            }
        }
    }
}

/// The view next to `from` in `direction`: the nearest one lying that way,
/// preferring the one most in line with it.
pub(crate) fn neighbor(rects: &[(u32, Rect)], from: u32, direction: Direction) -> Option<u32> {
    let (_, here) = rects.iter().find(|(id, _)| *id == from)?;
    let (cx, cy) = here.center();
    rects
        .iter()
        .filter(|(id, _)| *id != from)
        .filter(|(_, r)| match direction {
            Direction::Left => r.x + r.width <= here.x,
            Direction::Right => r.x >= here.x + here.width,
            Direction::Up => r.y + r.height <= here.y,
            Direction::Down => r.y >= here.y + here.height,
        })
        .min_by_key(|(_, r)| {
            let (x, y) = r.center();
            let (along, across) = match direction {
                Direction::Left | Direction::Right => ((x - cx).abs(), (y - cy).abs()),
                Direction::Up | Direction::Down => ((y - cy).abs(), (x - cx).abs()),
            };
            (along, across)
        })
        .map(|(id, _)| *id)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: Rect = Rect {
        x: 0,
        y: 0,
        width: 21,
        height: 11,
    };

    #[test]
    fn splits_side_by_side_and_above_each_other() {
        let mut tree = Splits::Leaf(1);
        tree.split(1, 2, true);
        tree.split(2, 3, false);
        assert_eq!(tree.leaves(), [1, 2, 3]);
        let (rects, seps) = tree.layout(AREA);
        assert_eq!(
            rects,
            [
                (
                    1,
                    Rect {
                        x: 0,
                        y: 0,
                        width: 10,
                        height: 11
                    }
                ),
                (
                    2,
                    Rect {
                        x: 11,
                        y: 0,
                        width: 10,
                        height: 5
                    }
                ),
                (
                    3,
                    Rect {
                        x: 11,
                        y: 6,
                        width: 10,
                        height: 5
                    }
                ),
            ]
        );
        assert_eq!(
            seps,
            [
                Separator::Column {
                    x: 10,
                    y: 0,
                    height: 11
                },
                Separator::Row {
                    x: 11,
                    y: 5,
                    width: 10,
                    view: 2
                },
            ]
        );
    }

    #[test]
    fn same_way_splits_do_not_nest() {
        let mut tree = Splits::Leaf(1);
        tree.split(1, 2, true);
        tree.split(1, 3, true);
        assert_eq!(
            tree,
            Splits::Split {
                side_by_side: true,
                children: vec![Splits::Leaf(1), Splits::Leaf(3), Splits::Leaf(2)],
            }
        );
    }

    #[test]
    fn removing_merges_what_is_left() {
        let mut tree = Splits::Leaf(1);
        tree.split(1, 2, true);
        tree.split(2, 3, false);
        tree.remove(3);
        assert_eq!(
            tree,
            Splits::Split {
                side_by_side: true,
                children: vec![Splits::Leaf(1), Splits::Leaf(2)],
            }
        );
        tree.remove(1);
        assert_eq!(tree, Splits::Leaf(2));
        tree.remove(2);
        assert_eq!(tree, Splits::Leaf(2), "the last view stays");
    }

    #[test]
    fn neighbors_lie_in_the_direction() {
        let mut tree = Splits::Leaf(1);
        tree.split(1, 2, true);
        tree.split(2, 3, false);
        let (rects, _) = tree.layout(AREA);
        assert_eq!(neighbor(&rects, 1, Direction::Right), Some(2));
        assert_eq!(neighbor(&rects, 3, Direction::Left), Some(1));
        assert_eq!(neighbor(&rects, 2, Direction::Down), Some(3));
        assert_eq!(neighbor(&rects, 3, Direction::Up), Some(2));
        assert_eq!(neighbor(&rects, 1, Direction::Left), None);
    }
}
