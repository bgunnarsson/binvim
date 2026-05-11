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
        // csharpier handles .cs cleanly. For .cshtml / .razor csharpier 1.x
        // says "Is an unsupported file type" and exits 0 — try it anyway
        // (a future csharpier may add Razor support), then fall back to the
        // .editorconfig-driven indent reflow if it punted.
        "cs" => match run_csharpier(path, source)? {
            Some(formatted) => Ok(formatted),
            None => Ok(source.to_string()),
        },
        "cshtml" | "razor" => match run_csharpier(path, source)? {
            Some(formatted) => Ok(formatted),
            None => apply_editorconfig_indent(path, source),
        },
        "go" => run_gofmt(source),
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
/// Returns:
/// - `Ok(Some(text))` — csharpier formatted the file; here's the result
/// - `Ok(None)` — csharpier ran but didn't recognise this file type
///   (it prints `Warning … - Is an unsupported file type.` and exits 0
///   for .cshtml / .razor in 1.x). Lets the caller fall back to a
///   different formatter.
/// - `Err(msg)` — a real failure: csharpier missing, parse error,
///   non-zero exit, etc.
fn run_csharpier(path: &Path, source: &str) -> Result<Option<String>, String> {
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
    let outcome = match &result {
        Ok(CsharpierOutcome::Formatted) => std::fs::read_to_string(&temp)
            .map(Some)
            .map_err(|e| format!("read temp: {e}")),
        Ok(CsharpierOutcome::Unsupported) => Ok(None),
        Err(e) => Err(e.clone()),
    };
    let _ = std::fs::remove_file(&temp);
    outcome
}

enum CsharpierOutcome {
    Formatted,
    Unsupported,
}

/// Normalise leading whitespace per the project's `.editorconfig` and
/// honour `charset = utf-8-bom` by ensuring a BOM is (or isn't) on the
/// front of the buffer. Used for `.cshtml` / `.razor` — csharpier doesn't
/// support those file types, but at minimum users expect their indent
/// settings to take effect when they save.
///
/// Only *leading* tabs / spaces are reflowed — tabs inside string
/// literals or in the middle of a line are left alone, since they're
/// almost always intentional in Razor markup.
fn apply_editorconfig_indent(path: &Path, source: &str) -> Result<String, String> {
    use crate::editorconfig::{EditorConfig, IndentStyle};
    let cfg = EditorConfig::detect(path);
    let tab_width = cfg.tab_width.max(1);
    let indent_size = cfg.indent_size.max(1);
    let target_unit = match cfg.indent_style {
        IndentStyle::Spaces => " ".repeat(indent_size),
        IndentStyle::Tabs => "\t".to_string(),
    };

    // BOM handling — only act when an .editorconfig section explicitly
    // applies. We can't tell from `EditorConfig` whether `charset` was set
    // on this file (the struct doesn't carry the field yet), so the BOM
    // pass is left to a follow-up; for now we preserve whatever's on the
    // front of `source`.
    let mut out = String::with_capacity(source.len());
    for line in source.split_inclusive('\n') {
        let (leading, rest) = split_leading_indent(line);
        let visual = leading
            .chars()
            .map(|c| if c == '\t' { tab_width } else { 1 })
            .sum::<usize>();
        let levels = visual / indent_size;
        let extra = visual % indent_size;
        for _ in 0..levels {
            out.push_str(&target_unit);
        }
        // Stray columns that don't form a full indent level — keep them as
        // plain spaces so we don't accidentally fuse partial indents into
        // a tab the user didn't intend.
        if extra > 0 {
            for _ in 0..extra {
                out.push(' ');
            }
        }
        out.push_str(rest);
    }
    Ok(out)
}

/// Split a line into (leading whitespace, rest). The leading run includes
/// any tabs and spaces; everything from the first non-whitespace char on
/// is the rest. Newlines stay in `rest` so `split_inclusive('\n')` output
/// round-trips cleanly.
fn split_leading_indent(line: &str) -> (&str, &str) {
    let mut idx = 0;
    for (i, c) in line.char_indices() {
        if c == ' ' || c == '\t' {
            idx = i + c.len_utf8();
        } else {
            return (&line[..idx], &line[idx..]);
        }
    }
    (&line[..idx], &line[idx..])
}

/// Format Go source via `gofmt` (or `goimports` when it's on PATH, which
/// also organises imports). Both read stdin and write formatted text to
/// stdout, so no temp file is needed — and neither tool consults
/// project-relative config, so we don't need `path` for resolution.
fn run_gofmt(source: &str) -> Result<String, String> {
    let (bin, label) = if let Some(p) = find_on_path("goimports") {
        (p, "goimports")
    } else if let Some(p) = find_on_path("gofmt") {
        (p, "gofmt")
    } else {
        return Err(
            "gofmt not found — install Go (gofmt ships with it) or run `go install golang.org/x/tools/cmd/goimports@latest`"
                .into(),
        );
    };

    let mut child = Command::new(&bin)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to spawn {label}: {e}"))?;
    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| format!("{label} stdin missing"))?;
        stdin
            .write_all(source.as_bytes())
            .map_err(|e| format!("write to {label} stdin: {e}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|e| format!("{label} wait: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // gofmt's error format: `<stdin>:LINE:COL: message` — keep the
        // first few lines so the user sees what's wrong.
        let cleaned: Vec<String> = stderr
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
        let code = output.status.code().map(|c| c.to_string()).unwrap_or_else(|| "?".into());
        return Err(format!("{label} exit {code}: {msg}"));
    }
    String::from_utf8(output.stdout).map_err(|e| format!("{label} stdout not utf-8: {e}"))
}

/// Resolve a binary by name on `$PATH`. Returns the first match.
fn find_on_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var("PATH").ok()?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn csharpier_format_inplace(csharpier: &Path, file: &Path) -> Result<CsharpierOutcome, String> {
    let mut child = Command::new(csharpier)
        .arg("format")
        .arg("--no-cache")
        .arg(file)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to spawn csharpier: {e}"))?;
    let mut stdout_buf = String::new();
    if let Some(mut s) = child.stdout.take() {
        let _ = s.read_to_string(&mut stdout_buf);
    }
    let mut stderr_buf = String::new();
    if let Some(mut s) = child.stderr.take() {
        let _ = s.read_to_string(&mut stderr_buf);
    }
    let status = child.wait().map_err(|e| format!("csharpier wait: {e}"))?;
    if !status.success() {
        let cleaned: Vec<String> = stderr_buf
            .lines()
            .chain(stdout_buf.lines())
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
    // csharpier 1.x prints `Warning <path> - Is an unsupported file type.`
    // and exits 0 when it doesn't recognise the file (everything besides
    // .cs in this version). That's the only signal we get that the run
    // was a no-op — forward it so the caller can pick a fallback path.
    if stdout_buf.contains("Is an unsupported file type")
        || stderr_buf.contains("Is an unsupported file type")
    {
        return Ok(CsharpierOutcome::Unsupported);
    }
    Ok(CsharpierOutcome::Formatted)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a temp dir with a `.editorconfig` and a target file, return
    /// the file's path. Caller owns the TempDir-like cleanup via the
    /// returned `tempfile::TempDir` substitute we implement inline (no
    /// extra dep needed — `std::env::temp_dir()` + a manual unique name).
    fn scratch(editorconfig: &str, target_name: &str, body: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "binvim-fmt-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join(".editorconfig"), editorconfig).unwrap();
        let target = root.join(target_name);
        std::fs::write(&target, body).unwrap();
        target
    }

    #[test]
    fn cshtml_tabs_become_spaces_per_editorconfig() {
        let ec = "root = true\n[*.{cshtml,html}]\nindent_style = space\nindent_size = 4\n";
        let target = scratch(ec, "view.cshtml", "");
        let src = "@{\n\tLayout = \"Master.cshtml\";\n\tvar x = 1;\n}\n<div>\n\t<a>hi</a>\n</div>\n";
        let out = apply_editorconfig_indent(&target, src).expect("ok");
        let expected = "@{\n    Layout = \"Master.cshtml\";\n    var x = 1;\n}\n<div>\n    <a>hi</a>\n</div>\n";
        assert_eq!(out, expected);
        let _ = std::fs::remove_dir_all(target.parent().unwrap());
    }

    #[test]
    fn cshtml_nested_tabs_become_spaces_per_level() {
        let ec = "root = true\n[*.cshtml]\nindent_style = space\nindent_size = 4\n";
        let target = scratch(ec, "deep.cshtml", "");
        let src = "<a>\n\t<b>\n\t\t<c>\n\t\t\t<d/>\n\t\t</c>\n\t</b>\n</a>\n";
        let out = apply_editorconfig_indent(&target, src).expect("ok");
        assert!(out.contains("    <b>"));
        assert!(out.contains("        <c>"));
        assert!(out.contains("            <d/>"));
        assert!(!out.contains('\t'), "no tabs left in output");
        let _ = std::fs::remove_dir_all(target.parent().unwrap());
    }

    #[test]
    fn cshtml_spaces_can_round_trip_to_tabs() {
        // Reverse direction: 4-space indents → 1 tab per level.
        let ec = "root = true\n[*.cshtml]\nindent_style = tab\nindent_size = 4\ntab_width = 4\n";
        let target = scratch(ec, "view.cshtml", "");
        let src = "<a>\n    <b>\n        <c/>\n    </b>\n</a>\n";
        let out = apply_editorconfig_indent(&target, src).expect("ok");
        assert!(out.contains("\t<b>"));
        assert!(out.contains("\t\t<c/>"));
        let _ = std::fs::remove_dir_all(target.parent().unwrap());
    }

    #[test]
    fn cshtml_keeps_inner_whitespace() {
        // Only LEADING whitespace gets reflowed — internal alignment is left alone.
        let ec = "root = true\n[*.cshtml]\nindent_style = space\nindent_size = 4\n";
        let target = scratch(ec, "view.cshtml", "");
        let src = "\t<a\thref=\"x\">y</a>\n";
        let out = apply_editorconfig_indent(&target, src).expect("ok");
        assert_eq!(out, "    <a\thref=\"x\">y</a>\n");
        let _ = std::fs::remove_dir_all(target.parent().unwrap());
    }
}
