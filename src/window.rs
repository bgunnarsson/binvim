//! A `Window` is a view onto a buffer — its own cursor, viewport, and
//! Visual-mode anchor. Until splits land, `App` carries exactly one of
//! these, but the data is already separated so the move to a multi-pane
//! layout (a tree of `Window` leaves) is a structural change to `App`
//! and not a re-shuffle of every cursor/viewport call site.
//!
//! Anything that's intrinsic to the file (undo history, fold ranges,
//! marks, blame, highlight cache) stays in `BufferStash`, not here —
//! multiple windows showing the same buffer share that state.

use crate::cursor::Cursor;

#[derive(Default, Debug, Clone)]
pub struct Window {
    pub cursor: Cursor,
    pub view_top: usize,
    /// Visual columns hidden off the left edge of the buffer area.
    pub view_left: usize,
    pub visual_anchor: Option<Cursor>,
}
