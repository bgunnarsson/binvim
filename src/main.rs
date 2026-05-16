use anyhow::Result;
use std::env;
use std::path::PathBuf;

mod app;
mod buffer;
mod command;
mod config;
mod crash;
mod cursor;
mod dap;
mod editorconfig;
mod format;
mod git;
mod lang;
mod layout;
mod lsp;
mod markdown_render;
mod mode;
mod motion;
mod parser;
mod picker;
mod render;
mod session;
mod text_object;
mod undo;
mod window;

fn main() -> Result<()> {
    // Install the panic hook *before* anything that touches the
    // terminal. A panic inside the event loop would otherwise leave
    // the user's terminal in raw mode with the alt-screen active, no
    // cursor visible, and no input echo — recovering means `stty sane`
    // in a different terminal, which is a miserable UX. The hook
    // restores the terminal, writes a crash log to
    // ~/.cache/binvim/crash/, and prints the path to stderr.
    crash::install_panic_hook();
    let path = env::args().nth(1).map(PathBuf::from);
    let mut app = app::App::new(path)?;
    app.run()
}
