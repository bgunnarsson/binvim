//! Panic hook + terminal-restore helper. Installed once at process
//! start so any panic in the event loop (or any background thread)
//! leaves the terminal usable and writes a diagnostic log instead of
//! leaving the user staring at a wedged TTY with no input echo.
//!
//! The hook is intentionally light: it doesn't dereference any
//! editor state (App lives behind `&mut self` borrows the unwinder
//! can't safely access mid-frame) — just the panic message,
//! location, backtrace, and a wall-clock timestamp. That's enough
//! to file a useful bug report. Buffer / cursor / mode state would
//! be nice but requires a thread-local or static handle into App
//! and isn't worth the indirection for v1.

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Install a global panic hook that:
///   1. Best-effort restores the terminal (disable raw mode, leave
///      alt screen, show cursor, drop kitty keyboard flags). Same
///      sequences `TerminalGuard::Drop` emits, but inlined here
///      because the unwinder may run before any `Drop` does.
///   2. Writes the panic info + backtrace + timestamp to
///      `~/.cache/binvim/crash/<unix-ts>.log`.
///   3. Prints "binvim crashed — log: <path>" to stderr so the user
///      knows where the diagnostic ended up.
///   4. Chains to the previous hook so cargo's default backtrace
///      printing still happens on debug builds.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal_best_effort();
        let log_path = match write_crash_log(info) {
            Some(p) => p,
            None => {
                // Couldn't write to disk — fall through to the default
                // hook so the user at least sees the panic message in
                // the now-usable terminal.
                let _ = writeln!(
                    std::io::stderr(),
                    "binvim crashed — could not write crash log to disk"
                );
                previous(info);
                return;
            }
        };
        let _ = writeln!(
            std::io::stderr(),
            "\nbinvim crashed — log: {}\n",
            log_path.display()
        );
        previous(info);
    }));
}

/// Emit the CSI sequences that `TerminalGuard::Drop` normally
/// handles. Wrapped in `let _ = …` everywhere because we're already
/// in a panic — failing to disable raw mode is regrettable but not
/// recoverable. Order matters: pop keyboard flags before leaving the
/// alt screen, leave the alt screen before disabling raw mode, so
/// each step still has the input modes it expects.
fn restore_terminal_best_effort() {
    use crossterm::{
        cursor::{SetCursorStyle, Show},
        event::{DisableMouseCapture, PopKeyboardEnhancementFlags},
        execute,
        terminal::{disable_raw_mode, LeaveAlternateScreen},
    };
    let mut stdout = std::io::stdout();
    let _ = execute!(stdout, PopKeyboardEnhancementFlags);
    let _ = execute!(
        stdout,
        DisableMouseCapture,
        SetCursorStyle::DefaultUserShape,
        Show,
        LeaveAlternateScreen
    );
    let _ = disable_raw_mode();
}

fn write_crash_log(info: &std::panic::PanicHookInfo<'_>) -> Option<PathBuf> {
    let dir = crash_dir()?;
    let _ = fs::create_dir_all(&dir);
    let unix_ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("{unix_ts}.log"));
    let mut file = fs::File::create(&path).ok()?;

    let _ = writeln!(file, "binvim crash log");
    let _ = writeln!(file, "================");
    let _ = writeln!(file, "version: {}", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(file, "unix_ts: {unix_ts}");

    // The payload is usually a `&str` or `String`; both work.
    let payload = info
        .payload()
        .downcast_ref::<&'static str>()
        .copied()
        .map(|s| s.to_string())
        .or_else(|| info.payload().downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(non-string panic payload)".into());
    let _ = writeln!(file, "panic: {payload}");

    if let Some(loc) = info.location() {
        let _ = writeln!(
            file,
            "at: {}:{}:{}",
            loc.file(),
            loc.line(),
            loc.column()
        );
    }

    // Capture a backtrace — RUST_BACKTRACE=1 inherited from the env
    // controls verbosity. `Backtrace::force_capture` ignores the env
    // var and always captures, which is what we want from a crash
    // log; debugging without one is meaningfully harder.
    let backtrace = std::backtrace::Backtrace::force_capture();
    let _ = writeln!(file, "\nbacktrace:");
    let _ = writeln!(file, "{backtrace}");
    Some(path)
}

fn crash_dir() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".cache/binvim/crash"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Catches the panic, runs the hook, and asserts the log file
    /// exists with the panic payload + location + a backtrace.
    /// `catch_unwind` swallows the unwind so the test framework
    /// stays alive.
    #[test]
    fn panic_hook_writes_log_with_payload() {
        // Point HOME at a scratch dir so we don't trash the real
        // ~/.cache/binvim/crash. set_var is unsafe under the new
        // edition rules; the unsafety is the cross-thread race
        // window which we ignore in a single-threaded test.
        let tmp = std::env::temp_dir().join("binvim_crash_test_panic");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        unsafe { std::env::set_var("HOME", &tmp); }

        install_panic_hook();
        let needle = "binvim_crash_test_unique_payload_marker";
        let result = std::panic::catch_unwind(|| {
            panic!("{}", needle);
        });
        assert!(result.is_err());

        let dir = tmp.join(".cache/binvim/crash");
        let mut entries: Vec<PathBuf> = fs::read_dir(&dir)
            .expect("crash dir should exist")
            .flatten()
            .map(|e| e.path())
            .collect();
        entries.sort();
        let log = entries.last().expect("at least one crash log");
        let body = fs::read_to_string(log).unwrap();
        assert!(body.contains("binvim crash log"), "missing header: {body}");
        assert!(body.contains(needle), "payload missing from log: {body}");
        assert!(body.contains("at:"), "location missing from log: {body}");
        // backtrace section presence — exact frames are platform-dependent
        // and not worth pinning, but the section header should be there.
        assert!(body.contains("backtrace:"), "backtrace section missing: {body}");

        let _ = fs::remove_dir_all(&tmp);
    }
}
