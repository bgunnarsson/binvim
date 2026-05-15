//! A `Window` is a view onto a buffer — its own cursor, viewport, and
//! Visual-mode anchor, plus the index of which buffer it's currently
//! looking at. Anything intrinsic to the file (undo history, fold
//! ranges, marks, blame, highlight cache) lives in `BufferStash`;
//! multiple windows showing the same buffer share that state.
//!
//! The active window's `Window` is held live on `App.window`; inactive
//! windows are stashed in `App.windows` and swapped in when focus moves.
//! Layout (split tree) is owned separately by `App.layout` and indexes
//! windows by `WindowId`.

use crate::cursor::Cursor;

#[derive(Default, Debug, Clone)]
pub struct Window {
    /// Index into `App.buffers` of the buffer this window is showing.
    /// On focus changes the active window's `buffer_idx` drives whether
    /// the live `App.buffer` (and per-buffer stashes like history, folds,
    /// highlight cache) needs to be swapped.
    pub buffer_idx: usize,
    pub cursor: Cursor,
    pub view_top: usize,
    /// Visual columns hidden off the left edge of the buffer area.
    pub view_left: usize,
    pub visual_anchor: Option<Cursor>,
}
