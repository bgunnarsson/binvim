//! Window-split tree. `Layout` owns a binary tree of `LayoutNode`s
//! whose leaves are `WindowId`s; the actual `Window` view state
//! (cursor, viewport, buffer index) lives in `App` — the live active
//! window inline on `App.window`, the rest stashed in
//! `App.windows: HashMap<WindowId, Window>`.
//!
//! `Layout::partition` walks the tree against a parent `Rect` and
//! returns one `(WindowId, Rect)` per leaf. `Layout::focus_neighbor`
//! uses those same rectangles to pick the window directly adjacent in
//! `h`/`j`/`k`/`l` from the focused one — geometric, not tree-order,
//! so the user's spatial intuition matches what they see on screen.

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct WindowId(pub u32);

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SplitDir {
    /// Children stacked vertically — split bar is horizontal. `<C-w>s`.
    Horizontal,
    /// Children side-by-side — split bar is vertical. `<C-w>v`.
    Vertical,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum FocusDir {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone)]
pub enum LayoutNode {
    Leaf(WindowId),
    Split {
        dir: SplitDir,
        /// Fraction of the parent rect allocated to `a`. `1.0 - ratio`
        /// goes to `b`. Clamped to `[0.1, 0.9]` at apply time so a
        /// pane can't vanish entirely from rounding.
        ratio: f32,
        a: Box<LayoutNode>,
        b: Box<LayoutNode>,
    },
}

#[derive(Debug, Clone)]
pub struct Layout {
    pub root: LayoutNode,
    next_id: u32,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

impl Layout {
    pub fn new() -> (Self, WindowId) {
        let id = WindowId(0);
        (
            Self {
                root: LayoutNode::Leaf(id),
                next_id: 1,
            },
            id,
        )
    }

    pub fn alloc_id(&mut self) -> WindowId {
        let id = WindowId(self.next_id);
        self.next_id += 1;
        id
    }

    /// Replace the leaf for `focus` with a Split whose first child is
    /// the original leaf and whose second child is `new_id`. The new
    /// window inherits half the focused window's space.
    ///
    /// Returns `true` on success, `false` if `focus` wasn't found.
    pub fn split(&mut self, focus: WindowId, dir: SplitDir, new_id: WindowId) -> bool {
        Self::split_in(&mut self.root, focus, dir, new_id)
    }

    fn split_in(node: &mut LayoutNode, focus: WindowId, dir: SplitDir, new_id: WindowId) -> bool {
        match node {
            LayoutNode::Leaf(id) if *id == focus => {
                let old = std::mem::replace(node, LayoutNode::Leaf(WindowId(0)));
                *node = LayoutNode::Split {
                    dir,
                    ratio: 0.5,
                    a: Box::new(old),
                    b: Box::new(LayoutNode::Leaf(new_id)),
                };
                true
            }
            LayoutNode::Leaf(_) => false,
            LayoutNode::Split { a, b, .. } => {
                Self::split_in(a, focus, dir, new_id) || Self::split_in(b, focus, dir, new_id)
            }
        }
    }

    /// Remove the leaf for `target` and collapse the sibling up.
    /// Returns the `WindowId` whose pane absorbed the closed window's
    /// space (the natural new-focus target). Returns `None` if `target`
    /// is the last remaining window or wasn't found.
    pub fn close(&mut self, target: WindowId) -> Option<WindowId> {
        // Single-leaf root means there's nothing left to collapse into.
        if let LayoutNode::Leaf(id) = &self.root {
            if *id == target {
                return None;
            }
        }
        Self::close_in(&mut self.root, target)
    }

    fn close_in(node: &mut LayoutNode, target: WindowId) -> Option<WindowId> {
        if let LayoutNode::Split { a, b, .. } = node {
            // Direct child is the target — collapse the other child up.
            let drop_a = matches!(a.as_ref(), LayoutNode::Leaf(id) if *id == target);
            let drop_b = matches!(b.as_ref(), LayoutNode::Leaf(id) if *id == target);
            if drop_a || drop_b {
                let keep = if drop_a {
                    std::mem::replace(b.as_mut(), LayoutNode::Leaf(WindowId(0)))
                } else {
                    std::mem::replace(a.as_mut(), LayoutNode::Leaf(WindowId(0)))
                };
                let absorbed_into = Self::first_leaf(&keep);
                *node = keep;
                return Some(absorbed_into);
            }
            // Otherwise recurse.
            if let Some(id) = Self::close_in(a, target) {
                return Some(id);
            }
            if let Some(id) = Self::close_in(b, target) {
                return Some(id);
            }
        }
        None
    }

    /// Collect every window id present in the tree.
    pub fn ids(&self) -> Vec<WindowId> {
        let mut out = Vec::new();
        Self::collect_ids(&self.root, &mut out);
        out
    }

    fn collect_ids(node: &LayoutNode, out: &mut Vec<WindowId>) {
        match node {
            LayoutNode::Leaf(id) => out.push(*id),
            LayoutNode::Split { a, b, .. } => {
                Self::collect_ids(a, out);
                Self::collect_ids(b, out);
            }
        }
    }

    /// Walk the tree against `rect` and return one `(id, rect)` per
    /// leaf. Split bars consume one cell on the major axis between
    /// children — for a vertical split the boundary column is owned by
    /// neither child and is painted by the renderer as a divider.
    pub fn partition(&self, rect: Rect) -> Vec<(WindowId, Rect)> {
        let mut out = Vec::new();
        Self::partition_in(&self.root, rect, &mut out);
        out
    }

    fn partition_in(node: &LayoutNode, rect: Rect, out: &mut Vec<(WindowId, Rect)>) {
        match node {
            LayoutNode::Leaf(id) => out.push((*id, rect)),
            LayoutNode::Split { dir, ratio, a, b } => {
                let (ra, rb) = Self::child_rects(*dir, *ratio, rect);
                Self::partition_in(a, ra, out);
                Self::partition_in(b, rb, out);
            }
        }
    }

    /// Pick the spatially-nearest neighbour of `focus` in `dir`. The
    /// algorithm: find `focus`'s rect, find the candidate rect that
    /// shares the most overlap along the perpendicular axis on the
    /// requested side, and return its id.
    pub fn focus_neighbor(
        &self,
        focus: WindowId,
        dir: FocusDir,
        rect: Rect,
    ) -> Option<WindowId> {
        let panes = self.partition(rect);
        let cur = panes.iter().find(|(id, _)| *id == focus)?.1;
        let mut best: Option<(WindowId, u16)> = None;
        for (id, r) in &panes {
            if *id == focus {
                continue;
            }
            // Adjacency check: the candidate must sit on the requested
            // side of `cur` and overlap on the perpendicular axis.
            let touches = match dir {
                FocusDir::Left => r.x + r.w <= cur.x,
                FocusDir::Right => r.x >= cur.x + cur.w,
                FocusDir::Up => r.y + r.h <= cur.y,
                FocusDir::Down => r.y >= cur.y + cur.h,
            };
            if !touches {
                continue;
            }
            let overlap = match dir {
                FocusDir::Left | FocusDir::Right => {
                    let lo = cur.y.max(r.y);
                    let hi = (cur.y + cur.h).min(r.y + r.h);
                    hi.saturating_sub(lo)
                }
                FocusDir::Up | FocusDir::Down => {
                    let lo = cur.x.max(r.x);
                    let hi = (cur.x + cur.w).min(r.x + r.w);
                    hi.saturating_sub(lo)
                }
            };
            if overlap == 0 {
                continue;
            }
            match best {
                None => best = Some((*id, overlap)),
                Some((_, prev)) if overlap > prev => best = Some((*id, overlap)),
                _ => {}
            }
        }
        best.map(|(id, _)| id)
    }

    /// `<C-w><N>>` / `<C-w><N><` / `<C-w><N>+` / `<C-w><N>-` —
    /// resize the focused window by `delta` cells along `axis`. Walks
    /// the tree to find the **deepest** ancestor of `focus` whose
    /// split direction matches `axis` and adjusts that node's ratio,
    /// converting cells to a fraction using the ancestor's own rect
    /// (so a `>10` near a small parent doesn't blow past the clamp
    /// just because the *whole screen* is wide). Sign rule: if focus
    /// is under the `a` child of that split, widening adds to the
    /// ratio; under `b`, widening subtracts. Returns `false` when
    /// the focused window has no ancestor along the requested axis
    /// (e.g. `<C-w>+` in a layout with only vertical splits).
    pub fn resize(&mut self, focus: WindowId, axis: SplitDir, delta: i32, area: Rect) -> bool {
        Self::resize_in(&mut self.root, focus, axis, delta, area)
    }

    fn resize_in(
        node: &mut LayoutNode,
        focus: WindowId,
        axis: SplitDir,
        delta: i32,
        rect: Rect,
    ) -> bool {
        let LayoutNode::Split { dir, ratio, a, b } = node else {
            return false;
        };
        let in_a = Self::contains(a, focus);
        let in_b = !in_a && Self::contains(b, focus);
        if !in_a && !in_b {
            return false;
        }
        let (ra, rb) = Self::child_rects(*dir, *ratio, rect);
        // Recurse first so we apply at the *deepest* matching ancestor.
        let (child_rect, child_node): (Rect, &mut LayoutNode) = if in_a {
            (ra, a.as_mut())
        } else {
            (rb, b.as_mut())
        };
        if Self::resize_in(child_node, focus, axis, delta, child_rect) {
            return true;
        }
        if *dir != axis {
            return false;
        }
        let total = match axis {
            SplitDir::Vertical => rect.w.saturating_sub(1) as f32,
            SplitDir::Horizontal => rect.h.saturating_sub(1) as f32,
        };
        if total <= 0.0 {
            return false;
        }
        let delta_r = delta as f32 / total;
        let signed = if in_a { delta_r } else { -delta_r };
        *ratio = (*ratio + signed).clamp(0.1, 0.9);
        true
    }

    fn contains(node: &LayoutNode, focus: WindowId) -> bool {
        match node {
            LayoutNode::Leaf(id) => *id == focus,
            LayoutNode::Split { a, b, .. } => {
                Self::contains(a, focus) || Self::contains(b, focus)
            }
        }
    }

    fn child_rects(dir: SplitDir, ratio: f32, rect: Rect) -> (Rect, Rect) {
        let r = ratio.clamp(0.1, 0.9);
        match dir {
            SplitDir::Vertical => {
                let usable = rect.w.saturating_sub(1);
                let aw = ((usable as f32) * r)
                    .round()
                    .max(1.0)
                    .min(usable as f32 - 1.0) as u16;
                let bw = usable.saturating_sub(aw);
                (
                    Rect { x: rect.x, y: rect.y, w: aw, h: rect.h },
                    Rect { x: rect.x + aw + 1, y: rect.y, w: bw, h: rect.h },
                )
            }
            SplitDir::Horizontal => {
                let usable = rect.h.saturating_sub(1);
                let ah = ((usable as f32) * r)
                    .round()
                    .max(1.0)
                    .min(usable as f32 - 1.0) as u16;
                let bh = usable.saturating_sub(ah);
                (
                    Rect { x: rect.x, y: rect.y, w: rect.w, h: ah },
                    Rect { x: rect.x, y: rect.y + ah + 1, w: rect.w, h: bh },
                )
            }
        }
    }

    /// Reset every split ratio to 0.5 — `<C-w>=` equivalent.
    pub fn equalize(&mut self) {
        Self::equalize_in(&mut self.root);
    }

    fn equalize_in(node: &mut LayoutNode) {
        if let LayoutNode::Split { ratio, a, b, .. } = node {
            *ratio = 0.5;
            Self::equalize_in(a);
            Self::equalize_in(b);
        }
    }

    /// Collapse everything down to a single leaf containing `keep` —
    /// `<C-w>o` equivalent. Returns every id that was discarded so the
    /// caller can drop their stashes.
    pub fn only(&mut self, keep: WindowId) -> Vec<WindowId> {
        let mut dropped = Vec::new();
        Self::collect_ids(&self.root, &mut dropped);
        dropped.retain(|id| *id != keep);
        self.root = LayoutNode::Leaf(keep);
        dropped
    }

    fn first_leaf(node: &LayoutNode) -> WindowId {
        match node {
            LayoutNode::Leaf(id) => *id,
            LayoutNode::Split { a, .. } => Self::first_leaf(a),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: u16, y: u16, w: u16, h: u16) -> Rect {
        Rect { x, y, w, h }
    }

    #[test]
    fn new_layout_has_one_leaf() {
        let (l, root) = Layout::new();
        assert_eq!(l.ids(), vec![root]);
        let panes = l.partition(rect(0, 0, 80, 24));
        assert_eq!(panes.len(), 1);
        assert_eq!(panes[0].0, root);
        assert_eq!(panes[0].1, rect(0, 0, 80, 24));
    }

    #[test]
    fn vertical_split_partitions_horizontally() {
        let (mut l, root) = Layout::new();
        let new = l.alloc_id();
        assert!(l.split(root, SplitDir::Vertical, new));
        let panes = l.partition(rect(0, 0, 81, 24));
        assert_eq!(panes.len(), 2);
        // A on the left, B on the right; 81 cols → 40 + divider + 40.
        let (id_a, ra) = &panes[0];
        let (id_b, rb) = &panes[1];
        assert_eq!(*id_a, root);
        assert_eq!(*id_b, new);
        assert_eq!(ra.x, 0);
        assert_eq!(rb.x, ra.w + 1);
        assert_eq!(ra.w + rb.w + 1, 81);
        assert_eq!(ra.h, 24);
        assert_eq!(rb.h, 24);
    }

    #[test]
    fn horizontal_split_partitions_vertically() {
        let (mut l, root) = Layout::new();
        let new = l.alloc_id();
        l.split(root, SplitDir::Horizontal, new);
        let panes = l.partition(rect(0, 0, 80, 25));
        let ra = panes[0].1;
        let rb = panes[1].1;
        assert_eq!(ra.x, 0);
        assert_eq!(rb.x, 0);
        assert_eq!(ra.y, 0);
        assert_eq!(rb.y, ra.h + 1);
        assert_eq!(ra.h + rb.h + 1, 25);
    }

    #[test]
    fn close_collapses_sibling_up() {
        let (mut l, root) = Layout::new();
        let new = l.alloc_id();
        l.split(root, SplitDir::Vertical, new);
        let new_focus = l.close(new).unwrap();
        assert_eq!(new_focus, root);
        assert_eq!(l.ids(), vec![root]);
    }

    #[test]
    fn close_last_window_refuses() {
        let (mut l, root) = Layout::new();
        assert!(l.close(root).is_none());
        assert_eq!(l.ids(), vec![root]);
    }

    #[test]
    fn focus_neighbor_left_right() {
        let (mut l, root) = Layout::new();
        let r = l.alloc_id();
        l.split(root, SplitDir::Vertical, r);
        let area = rect(0, 0, 81, 24);
        assert_eq!(l.focus_neighbor(root, FocusDir::Right, area), Some(r));
        assert_eq!(l.focus_neighbor(r, FocusDir::Left, area), Some(root));
        // No neighbour past the edge.
        assert_eq!(l.focus_neighbor(root, FocusDir::Left, area), None);
        assert_eq!(l.focus_neighbor(r, FocusDir::Right, area), None);
        // Vertical neighbour query in a horizontal-only layout finds nothing.
        assert_eq!(l.focus_neighbor(root, FocusDir::Up, area), None);
    }

    #[test]
    fn focus_neighbor_three_pane_grid() {
        // Layout: root | (top: a, bottom: b)
        // Right column is horizontally split — left column has two right-side neighbours.
        let (mut l, root) = Layout::new();
        let r = l.alloc_id();
        l.split(root, SplitDir::Vertical, r);
        let r_bottom = l.alloc_id();
        l.split(r, SplitDir::Horizontal, r_bottom);
        let area = rect(0, 0, 81, 25);
        // From root → Right: should pick whichever right-column pane has
        // more vertical overlap. Both are ~equal; first-with-max-overlap
        // wins (algorithm picks the top one since it's enumerated first
        // and tie-breaks on `>`).
        let pick = l.focus_neighbor(root, FocusDir::Right, area).unwrap();
        assert!(pick == r || pick == r_bottom);
        // From r → Down lands on r_bottom.
        assert_eq!(l.focus_neighbor(r, FocusDir::Down, area), Some(r_bottom));
        assert_eq!(l.focus_neighbor(r_bottom, FocusDir::Up, area), Some(r));
    }

    #[test]
    fn only_collapses_to_single_leaf() {
        let (mut l, root) = Layout::new();
        let r = l.alloc_id();
        l.split(root, SplitDir::Vertical, r);
        let b = l.alloc_id();
        l.split(r, SplitDir::Horizontal, b);
        let dropped = l.only(root);
        assert_eq!(dropped.len(), 2);
        assert!(dropped.contains(&r));
        assert!(dropped.contains(&b));
        assert_eq!(l.ids(), vec![root]);
    }

    #[test]
    fn resize_widens_focus_in_left_pane() {
        let (mut l, root) = Layout::new();
        let r = l.alloc_id();
        l.split(root, SplitDir::Vertical, r);
        let area = rect(0, 0, 101, 24); // usable width = 100
        assert!(l.resize(root, SplitDir::Vertical, 10, area));
        let LayoutNode::Split { ratio, .. } = &l.root else {
            panic!("expected split root");
        };
        // Focus in `a` (left) → widening adds; 10/100 = +0.1, base 0.5.
        assert!((ratio - 0.6).abs() < 1e-5, "ratio = {ratio}");
    }

    #[test]
    fn resize_widens_focus_in_right_pane_by_lowering_ratio() {
        let (mut l, root) = Layout::new();
        let r = l.alloc_id();
        l.split(root, SplitDir::Vertical, r);
        let area = rect(0, 0, 101, 24);
        assert!(l.resize(r, SplitDir::Vertical, 10, area));
        let LayoutNode::Split { ratio, .. } = &l.root else {
            panic!("expected split root");
        };
        assert!((ratio - 0.4).abs() < 1e-5, "ratio = {ratio}");
    }

    #[test]
    fn resize_negative_delta_shrinks() {
        let (mut l, root) = Layout::new();
        let r = l.alloc_id();
        l.split(root, SplitDir::Vertical, r);
        let area = rect(0, 0, 101, 24);
        assert!(l.resize(root, SplitDir::Vertical, -10, area));
        let LayoutNode::Split { ratio, .. } = &l.root else {
            panic!("expected split root");
        };
        assert!((ratio - 0.4).abs() < 1e-5, "ratio = {ratio}");
    }

    #[test]
    fn resize_ignores_mismatched_axis() {
        // Vertical-only split; <C-w>+ (horizontal axis) finds no
        // matching ancestor and reports no-op.
        let (mut l, root) = Layout::new();
        let r = l.alloc_id();
        l.split(root, SplitDir::Vertical, r);
        let area = rect(0, 0, 80, 24);
        assert!(!l.resize(root, SplitDir::Horizontal, 5, area));
    }

    #[test]
    fn resize_picks_deepest_matching_ancestor() {
        // Outer vertical split; inner pane is split vertically again.
        // Focus on the innermost-right leaf — resizing should adjust
        // the *inner* ratio, not the outer one.
        let (mut l, root) = Layout::new();
        let mid = l.alloc_id();
        l.split(root, SplitDir::Vertical, mid);
        let inner_right = l.alloc_id();
        l.split(mid, SplitDir::Vertical, inner_right);
        let area = rect(0, 0, 101, 24);
        // Snapshot outer ratio so we can confirm it didn't move.
        let LayoutNode::Split { ratio: outer_before, b: outer_b, .. } = &l.root else {
            panic!("expected split root");
        };
        let outer_before = *outer_before;
        // Inner ratio sits on the right subtree.
        let inner_before = if let LayoutNode::Split { ratio, .. } = outer_b.as_ref() {
            *ratio
        } else {
            panic!("expected inner split");
        };
        assert!(l.resize(inner_right, SplitDir::Vertical, 5, area));
        let LayoutNode::Split { ratio: outer_after, b: outer_b, .. } = &l.root else {
            unreachable!();
        };
        assert!((outer_after - outer_before).abs() < 1e-6, "outer changed");
        let inner_after = if let LayoutNode::Split { ratio, .. } = outer_b.as_ref() {
            *ratio
        } else {
            unreachable!();
        };
        assert!(
            (inner_after - inner_before).abs() > 1e-6,
            "inner did not move",
        );
    }

    #[test]
    fn resize_clamps_to_visible_range() {
        let (mut l, root) = Layout::new();
        let r = l.alloc_id();
        l.split(root, SplitDir::Vertical, r);
        let area = rect(0, 0, 21, 24); // usable = 20
        // Widen left by 50 cells — would push ratio past 1.0; should
        // clamp to the layout's 0.9 ceiling.
        assert!(l.resize(root, SplitDir::Vertical, 50, area));
        let LayoutNode::Split { ratio, .. } = &l.root else {
            panic!("expected split root");
        };
        assert!((ratio - 0.9).abs() < 1e-5, "ratio = {ratio}");
    }

    #[test]
    fn equalize_resets_ratios() {
        let (mut l, root) = Layout::new();
        let r = l.alloc_id();
        l.split(root, SplitDir::Vertical, r);
        if let LayoutNode::Split { ratio, .. } = &mut l.root {
            *ratio = 0.8;
        }
        l.equalize();
        if let LayoutNode::Split { ratio, .. } = &l.root {
            assert!((ratio - 0.5).abs() < 1e-6);
        } else {
            panic!("expected split root");
        }
    }
}
