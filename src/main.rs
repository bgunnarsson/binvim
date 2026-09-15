use anyhow::Result;
use std::env;
use std::path::PathBuf;

mod android;
mod ansi;
mod app;
mod buffer;
mod code_lens_synth;
mod command;
mod config;
mod crash;
mod cursor;
mod dap;
mod editorconfig;
mod format;
mod git;
mod keymap;
mod lang;
mod layout;
mod lsp;
mod markdown_render;
mod mode;
mod motion;
mod package;
mod parser;
mod paths;
mod picker;
mod recover;
mod render;
mod session;
mod spell;
mod task;
mod terminal;
mod test;
mod text_object;
mod undo;
mod update;
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
    // The loop can end without a quit: a panic, or an error out of it (a draw
    // that failed). Buffers are still dirty then, so their recovery files are
    // brought up to date before the process goes. Signals are handled on
    // their own thread — see `App::spawn_signal_recovery`.
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| app.run())) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => {
            app.write_recovery_now();
            Err(e)
        }
        Err(panic) => {
            app.write_recovery_now();
            std::panic::resume_unwind(panic)
        }
    }
}
