//! Buffer-level formatters. Pipes the buffer through an external tool
//! (biome for JS/TS/JSON, csharpier for C# / Razor) and returns the
//! formatted text. The caller is responsible for replacing the buffer and
//! bookkeeping the history.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::lsp::find_node_modules_bin;

/// Pick a formatter for `path`'s extension and run it against `source`.
/// Returns the formatted text on success, or a short message describing the
/// failure (suitable for the status line).
pub fn format_buffer(path: &Path, source: &str) -> Result<String, String> {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    match ext {
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "json" | "jsonc" => {
            run_biome(path, source)
        }
        "cs" | "cshtml" | "razor" => run_csharpier(path, source),
        _ => Err(format!("no formatter configured for .{ext}")),
    }
}

/// Run biome against `source`, telling it the buffer's real path so it can
/// detect language and walk up to find `biome.json` itself. We resolve the
/// binary the same way the LSP does — closest `node_modules/.bin/biome` from
/// the file's directory — since biome doesn't support global installs.
fn run_biome(path: &Path, source: &str) -> Result<String, String> {
    let start = path.parent().unwrap_or(Path::new("."));
    let biome = find_node_modules_bin(start, "biome")
        .ok_or_else(|| "biome not found in node_modules".to_string())?;
    let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let stdin_path = canon.to_string_lossy().to_string();

    let mut child = Command::new(&biome)
        .arg("format")
        .arg(format!("--stdin-file-path={stdin_path}"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to spawn biome: {e}"))?;

    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| "biome stdin missing".to_string())?;
        stdin
            .write_all(source.as_bytes())
            .map_err(|e| format!("write to biome stdin: {e}"))?;
    }

    let output = child
        .wait_with_output()
        .map_err(|e| format!("biome wait: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // biome wraps real diagnostics in box-drawing chrome and prints the
        // file path on the first line. Strip that chrome and join the
        // payload lines so the user actually sees what's wrong.
        let cleaned: Vec<String> = stderr
            .lines()
            .map(|l| {
                l.trim_matches(|c: char| {
                    c.is_whitespace() || matches!(c, '━' | '│' | '╭' | '╮' | '╯' | '╰' | '┃')
                })
                .to_string()
            })
            .filter(|l| !l.is_empty())
            .take(6)
            .collect();
        let msg = if cleaned.is_empty() {
            "(no error output)".to_string()
        } else {
            cleaned.join(" / ")
        };
        let code = output.status.code().map(|c| c.to_string()).unwrap_or_else(|| "?".into());
        return Err(format!("biome exit {code}: {msg}"));
    }

    String::from_utf8(output.stdout).map_err(|e| format!("biome stdout not utf-8: {e}"))
}

/// Resolve the csharpier binary. Checks `$PATH` first (covers Homebrew /
/// custom installs), then falls back to the conventional dotnet global-tool
/// location at `~/.dotnet/tools/csharpier` (where `dotnet tool install -g
/// csharpier` lands by default).
fn find_csharpier() -> Option<PathBuf> {
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join("csharpier");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    let home = std::env::var("HOME").ok()?;
    let dotnet_tools = PathBuf::from(home).join(".dotnet/tools/csharpier");
    if dotnet_tools.is_file() {
        return Some(dotnet_tools);
    }
    None
}

/// Format C# / Razor source with csharpier. csharpier walks up from the
/// file's directory to find `.csharpierrc` and `.editorconfig`, so we land
/// the temp file next to the real one and use the original extension —
/// formatting in `/tmp` would miss project-level config.
///
/// csharpier's stdin contract changes between versions, so we go through a
/// temp file instead: write `source`, run `csharpier format <temp>` (which
/// edits the file in place), read the result back, then unlink. The temp
/// file name uses a `.binvim-format` infix so it's obvious in directory
/// listings if a crash ever leaves one behind.
fn run_csharpier(path: &Path, source: &str) -> Result<String, String> {
    let csharpier = find_csharpier().ok_or_else(|| {
        "csharpier not found — install with `dotnet tool install -g csharpier`".to_string()
    })?;
    let parent = path.parent().unwrap_or(Path::new("."));
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("cs");
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("buffer");
    let temp = parent.join(format!(
        ".{stem}.binvim-format.{pid}.{ext}",
        pid = std::process::id(),
    ));
    std::fs::write(&temp, source).map_err(|e| format!("write temp: {e}"))?;
    let result = csharpier_format_inplace(&csharpier, &temp);
    let formatted = match &result {
        Ok(()) => std::fs::read_to_string(&temp).map_err(|e| format!("read temp: {e}")),
        Err(e) => Err(e.clone()),
    };
    // Best-effort cleanup — leaving the temp around isn't fatal, but
    // there's no reason to keep it once we have the bytes.
    let _ = std::fs::remove_file(&temp);
    formatted
}

fn csharpier_format_inplace(csharpier: &Path, file: &Path) -> Result<(), String> {
    let mut child = Command::new(csharpier)
        .arg("format")
        .arg("--no-cache")
        .arg(file)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to spawn csharpier: {e}"))?;
    let mut stderr_buf = String::new();
    if let Some(mut s) = child.stderr.take() {
        let _ = s.read_to_string(&mut stderr_buf);
    }
    let status = child.wait().map_err(|e| format!("csharpier wait: {e}"))?;
    if !status.success() {
        let cleaned: Vec<String> = stderr_buf
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .take(4)
            .collect();
        let msg = if cleaned.is_empty() {
            "(no error output)".to_string()
        } else {
            cleaned.join(" / ")
        };
        let code = status.code().map(|c| c.to_string()).unwrap_or_else(|| "?".into());
        return Err(format!("csharpier exit {code}: {msg}"));
    }
    Ok(())
}
