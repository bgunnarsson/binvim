//! Buffer-level formatters. Pipes the buffer through an external tool
//! (currently just biome) and returns the formatted text. The caller is
//! responsible for replacing the buffer and bookkeeping the history.

use std::io::Write;
use std::path::Path;
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
