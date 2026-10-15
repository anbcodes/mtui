//! A draggable divider between two panes.
//!
//! The app keeps one `Divider` per split, asks it for the first pane's size
//! when laying out (`size`), and offers it every mouse event first (`mouse`):
//! pressing on the divider line and dragging moves it, until the release.

use crate::term::MouseKind;

#[derive(Default, Clone, Debug)]
pub struct Divider {
    /// The size the user dragged the first pane to; None = the app's default.
    pub size: Option<usize>,
    dragging: bool,
}

impl Divider {
    /// The first pane's size: what it was dragged to, else `auto`, within `lo..=hi`.
    pub fn size(&self, auto: usize, lo: usize, hi: usize) -> usize {
        self.size.unwrap_or(auto).clamp(lo, hi.max(lo))
    }

    pub fn dragging(&self) -> bool {
        self.dragging
    }

    /// Feed a mouse event. `at` is the pointer's position along the split
    /// (x for side-by-side panes, y for stacked ones) measured from the first
    /// pane's start, and `line` is where the divider is drawn, in the same
    /// units (normally the first pane's current size). Returns true when the
    /// event was the divider's, and the caller should not handle it further.
    pub fn mouse(&mut self, kind: MouseKind, at: usize, line: usize) -> bool {
        match kind {
            MouseKind::Press(0) if at == line => {
                self.dragging = true;
                true
            }
            MouseKind::Drag(0) if self.dragging => {
                self.size = Some(at);
                true
            }
            MouseKind::Release if self.dragging => {
                self.dragging = false;
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag() {
        let mut d = Divider::default();
        assert_eq!(d.size(20, 10, 40), 20);
        assert!(!d.mouse(MouseKind::Press(0), 5, 20));
        assert!(!d.mouse(MouseKind::Drag(0), 6, 20));
        assert!(d.mouse(MouseKind::Press(0), 20, 20));
        assert!(d.mouse(MouseKind::Drag(0), 30, 20));
        assert!(d.mouse(MouseKind::Drag(0), 90, 20));
        assert_eq!(d.size(20, 10, 40), 40);
        assert!(d.mouse(MouseKind::Drag(0), 3, 20));
        assert_eq!(d.size(20, 10, 40), 10);
        assert!(d.mouse(MouseKind::Release, 3, 20));
        assert!(!d.mouse(MouseKind::Drag(0), 30, 20));
        assert_eq!(d.size(20, 10, 40), 10);
    }
}
