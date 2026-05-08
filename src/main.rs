use anyhow::Result;
use std::env;
use std::path::PathBuf;

mod app;
mod buffer;
mod command;
mod cursor;
mod mode;
mod motion;
mod parser;
mod render;
mod text_object;
mod undo;

fn main() -> Result<()> {
    let path = env::args().nth(1).map(PathBuf::from);
    let mut app = app::App::new(path)?;
    app.run()
}
