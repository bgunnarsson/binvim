use crate::cursor::Cursor;
use ropey::Rope;

#[derive(Clone)]
pub struct Snapshot {
    pub rope: Rope,
    pub cursor: Cursor,
}

#[derive(Default, Clone)]
pub struct History {
    past: Vec<Snapshot>,
    future: Vec<Snapshot>,
}


impl History {
    pub fn new() -> Self {
        Self::default()
    }

    /// Save the current state before a mutation.
    pub fn record(&mut self, rope: &Rope, cursor: Cursor) {
        self.past.push(Snapshot { rope: rope.clone(), cursor });
        self.future.clear();
    }

    /// Undo: take the last recorded snapshot and push current onto redo stack.
    pub fn undo(&mut self, current_rope: &Rope, current_cursor: Cursor) -> Option<Snapshot> {
        let snap = self.past.pop()?;
        self.future.push(Snapshot {
            rope: current_rope.clone(),
            cursor: current_cursor,
        });
        Some(snap)
    }

    pub fn redo(&mut self, current_rope: &Rope, current_cursor: Cursor) -> Option<Snapshot> {
        let snap = self.future.pop()?;
        self.past.push(Snapshot {
            rope: current_rope.clone(),
            cursor: current_cursor,
        });
        Some(snap)
    }
}
