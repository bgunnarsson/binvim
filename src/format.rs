//! Buffer-level formatters. Pipes the buffer through an external tool
//! (biome for JS/TS/JSON, csharpier for C# / Razor, gofmt / goimports for
//! Go, ruff or black for Python, clang-format for C/C++, shfmt for shell,
//! stylua for Lua) and returns the formatted text. The caller is
//! responsible for replacing the buffer and bookkeeping the history.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::lsp::find_node_modules_bin;

/// Pick a formatter for `path`'s extension and run it against `source`.
/// Returns the formatted text on success, or a short message describing the
/// failure (suitable for the status line).
pub fn format_buffer(path: &Path, source: &str) -> Result<String, String> {
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
    let formatted = match ext {
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "json" | "jsonc" => run_biome(path, source),
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
        "py" | "pyi" => run_python(path, source),
        "c" | "h" | "cc" | "cpp" | "cxx" | "hh" | "hpp" | "hxx" | "c++" | "h++" => {
            run_clang_format(path, source)
        }
        "sh" | "bash" | "zsh" | "ksh" => run_shfmt(path, source),
        "lua" => run_stylua(path, source),
        // Prettier covers the file types biome doesn't (yet): Markdown,
        // Vue, Svelte, HTML, CSS preprocessor variants (biome 2.x only
        // formats plain CSS), YAML, GraphQL. Project-local
        // `node_modules/.bin/prettier` wins over the global install
        // when present.
        "md" | "markdown" | "mdx" | "vue" | "svelte" | "html" | "htm" | "css" | "scss" | "less"
        | "yaml" | "yml" | "graphql" | "gql" => run_prettier(path, source),
        "toml" => run_taplo(path, source),
        "rb" | "rake" | "gemspec" => run_rufo(source),
        "php" => run_php_cs_fixer(path, source),
        "java" => run_google_java_format(source),
        "zig" => run_zig_fmt(source),
        "nix" => run_nixfmt(source),
        "ex" | "exs" => run_mix_format(source),
        "kt" | "kts" => run_ktfmt(path, source),
        "sql" => run_sql_formatter(source),
        _ => Err(format!("no formatter configured for .{ext}")),
    }?;
    reject_empty(source, formatted)
}

/// Every runner takes a zero exit as success, whatever it printed. A tool
/// that exits 0 having written nothing — a broken pipe, an invocation it
/// misread — would otherwise replace the buffer with nothing, and the save
/// that ran it would write that to disk.
fn reject_empty(source: &str, formatted: String) -> Result<String, String> {
    if formatted.trim().is_empty() && !source.trim().is_empty() {
        return Err("formatter returned no output; buffer left unchanged".into());
    }
    Ok(formatted)
}

/// Run a stdin→stdout formatter and return its stdout. Used for the
/// "single binary, reads source on stdin, writes formatted source on
/// stdout" tools where there's no project-specific resolution to do.
/// Errors carry a few lines of stderr so the status line shows what
/// the tool actually complained about.
fn run_stdin_pipe(bin: &Path, args: &[&str], source: &str, label: &str) -> Result<String, String> {
    let mut child = Command::new(bin)
        .args(args)
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
        let msg = crate::package::clip_lines(&output.stderr, 4)
            .unwrap_or_else(|| "(no error output)".to_string());
        let code = output
            .status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "?".into());
        return Err(format!("{label} exit {code}: {msg}"));
    }
    String::from_utf8(output.stdout).map_err(|e| format!("{label} stdout not utf-8: {e}"))
}

/// `!` filters: `cmd` run by the user's shell (`cmd /C` on Windows), with
/// `input` on stdin.
pub(crate) fn filter_through_shell(cmd: &str, input: &str) -> Result<String, String> {
    let label = format!("!{cmd}");
    let (shell, args) = shell_for(cmd);
    run_stdin_pipe(Path::new(&shell), &args, input, &label)
}

/// The user's shell and the flag that hands it a command line — `$SHELL
/// -c`, or `cmd /C` on Windows — in one place, so every `!` agrees.
fn shell_for(cmd: &str) -> (String, [&str; 2]) {
    if cfg!(windows) {
        return ("cmd".to_string(), ["/C", cmd]);
    }
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "sh".into());
    (shell, ["-c", cmd])
}

/// What a shell command printed — its output, then its errors — and its
/// exit code when it failed.
pub(crate) struct ShellOutput {
    pub text: String,
    pub failed: Option<i32>,
}

/// `:!`, `:r !` and `:w !`: `cmd` run the way `!` filters run it, `input` on
/// its stdin. The input is written from a thread of its own, so a command
/// that prints before it has read everything can't leave both sides waiting.
pub(crate) fn run_shell(cmd: &str, input: &str) -> Result<ShellOutput, String> {
    let (shell, args) = shell_for(cmd);
    let mut child = Command::new(&shell)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to spawn {shell}: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("shell stdin missing")?;
    let input = input.to_string();
    // A command that stops reading early closes the pipe; nothing is lost by
    // not minding that.
    let writer = std::thread::spawn(move || stdin.write_all(input.as_bytes()));
    let output = child
        .wait_with_output()
        .map_err(|e| format!("!{cmd}: {e}"))?;
    let _ = writer.join();
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    let failed = (!output.status.success()).then(|| output.status.code().unwrap_or(-1));
    Ok(ShellOutput { text, failed })
}

/// Format Python via `ruff format` (preferred — single Rust binary,
/// fast, picks up `pyproject.toml` / `ruff.toml` automatically) with
/// `black` as a fallback when ruff isn't installed. Both read stdin
/// when given `-` and write the formatted source on stdout.
fn run_python(path: &Path, source: &str) -> Result<String, String> {
    let stdin_filename = path.to_string_lossy().to_string();
    if let Some(ruff) = find_on_path("ruff") {
        return run_stdin_pipe(
            &ruff,
            &["format", "-", &format!("--stdin-filename={stdin_filename}")],
            source,
            "ruff",
        );
    }
    if let Some(black) = find_on_path("black") {
        return run_stdin_pipe(&black, &["-q", "-"], source, "black");
    }
    Err("no Python formatter found — install `ruff` (preferred) or `black`".into())
}

/// Format C / C++ via `clang-format`. Walks up from the source file to
/// find `.clang-format` itself (`-assume-filename` is what tells it
/// which file the stdin payload belongs to).
fn run_clang_format(path: &Path, source: &str) -> Result<String, String> {
    let clang = find_on_path("clang-format")
        .ok_or_else(|| "clang-format not found — install with `brew install llvm` or `apt install clang-format`".to_string())?;
    let assume = format!("-assume-filename={}", path.to_string_lossy());
    run_stdin_pipe(&clang, &[&assume], source, "clang-format")
}

/// Format shell scripts via `shfmt`. The `-filename` flag tells shfmt
/// which dialect to expect (bash / posix / mksh / bats) based on the
/// extension — without it, .bash scripts get parsed in posix mode and
/// reject bashisms.
fn run_shfmt(path: &Path, source: &str) -> Result<String, String> {
    let shfmt = find_on_path("shfmt").ok_or_else(|| {
        "shfmt not found — install with `brew install shfmt` or `go install mvdan.cc/sh/v3/cmd/shfmt@latest`".to_string()
    })?;
    let filename = format!("-filename={}", path.to_string_lossy());
    run_stdin_pipe(&shfmt, &[&filename], source, "shfmt")
}

/// Format Lua via `stylua`. `--search-parent-directories` lets it find
/// `stylua.toml` walking up from the source file, the same way the LSP
/// resolves project config.
fn run_stylua(path: &Path, source: &str) -> Result<String, String> {
    let stylua = find_on_path("stylua").ok_or_else(|| {
        "stylua not found — install with `cargo install stylua` or `brew install stylua`"
            .to_string()
    })?;
    let stdin_filepath = format!("--stdin-filepath={}", path.to_string_lossy());
    run_stdin_pipe(
        &stylua,
        &["--search-parent-directories", &stdin_filepath, "-"],
        source,
        "stylua",
    )
}

/// Format Markdown / Vue / Svelte via Prettier. Walks up from the file
/// looking for `node_modules/.bin/prettier` (the same resolution biome
/// uses), falling back to a global install on `$PATH`. Svelte needs
/// `prettier-plugin-svelte` in the project's node_modules — Prettier
/// auto-loads it when the plugin is installed.
fn run_prettier(path: &Path, source: &str) -> Result<String, String> {
    let start = path.parent().unwrap_or(Path::new("."));
    let prettier_bin = find_node_modules_bin(start, "prettier")
        .map(PathBuf::from)
        .or_else(|| find_on_path("prettier"))
        .ok_or_else(|| {
            "prettier not found — install with `npm i -D prettier` in the project (or `npm i -g prettier`)"
                .to_string()
        })?;
    let stdin_filepath = format!("--stdin-filepath={}", path.to_string_lossy());
    run_stdin_pipe(&prettier_bin, &[&stdin_filepath], source, "prettier")
}

/// Format TOML via `taplo format -`. `-` is taplo's stdin sigil; it
/// writes the formatted source to stdout. taplo walks up from the cwd
/// looking for `.taplo.toml` / `taplo.toml` itself, so no extra args
/// are needed for project-specific style overrides.
fn run_taplo(path: &Path, source: &str) -> Result<String, String> {
    let taplo = find_on_path("taplo").ok_or_else(|| {
        "taplo not found — install with `cargo install taplo-cli --features lsp`".to_string()
    })?;
    // `--stdin-filepath` tells taplo which file the stdin payload
    // belongs to so project-relative config resolves correctly.
    let stdin_filepath = format!("--stdin-filepath={}", path.to_string_lossy());
    run_stdin_pipe(&taplo, &["format", &stdin_filepath, "-"], source, "taplo")
}

/// Format Ruby via `rufo -x`. `-x` is rufo's stdin mode — reads source
/// on stdin, writes the formatted result on stdout. Rubocop is the more
/// dominant Ruby tool overall, but its stdin output format mixes
/// diagnostics with the corrected source; rufo's narrower scope (pure
/// formatting, no linting) is a better fit for the editor save path.
fn run_rufo(source: &str) -> Result<String, String> {
    let rufo = find_on_path("rufo")
        .ok_or_else(|| "rufo not found — install with `gem install rufo`".to_string())?;
    run_stdin_pipe(&rufo, &["-x"], source, "rufo")
}

/// Format Java via `google-java-format -`. The bare `-` reads stdin
/// and writes the formatted source on stdout. Picks up
/// `--aosp` / `--skip-javadoc-formatting` etc. from environment if the
/// project's build config sets them, but for the editor save path we
/// pass nothing and let the defaults stand.
fn run_google_java_format(source: &str) -> Result<String, String> {
    let bin = find_on_path("google-java-format").ok_or_else(|| {
        "google-java-format not found — install with `brew install google-java-format`".to_string()
    })?;
    run_stdin_pipe(&bin, &["-"], source, "google-java-format")
}

/// Format Zig via `zig fmt --stdin`. Ships with the Zig toolchain so
/// any user with `zig` on PATH already has the formatter.
fn run_zig_fmt(source: &str) -> Result<String, String> {
    let zig = find_on_path("zig")
        .ok_or_else(|| "zig not found — install the Zig toolchain".to_string())?;
    run_stdin_pipe(&zig, &["fmt", "--stdin"], source, "zig fmt")
}

/// Format Nix via `nixfmt` (RFC 166's reference implementation) with
/// `alejandra` as a fallback. Both read from stdin and write to stdout
/// with no extra arguments.
fn run_nixfmt(source: &str) -> Result<String, String> {
    if let Some(nixfmt) = find_on_path("nixfmt") {
        return run_stdin_pipe(&nixfmt, &[], source, "nixfmt");
    }
    if let Some(alejandra) = find_on_path("alejandra") {
        return run_stdin_pipe(&alejandra, &["--quiet", "-"], source, "alejandra");
    }
    Err("no Nix formatter found — install `nixfmt` or `alejandra`".into())
}

/// Format Elixir via `mix format -`. The bare `-` is mix's stdin sigil.
/// `mix format` walks up from the cwd to find `.formatter.exs`, so
/// project-level style choices apply.
fn run_mix_format(source: &str) -> Result<String, String> {
    let mix = find_on_path("mix")
        .ok_or_else(|| "mix not found — install the Elixir toolchain".to_string())?;
    run_stdin_pipe(&mix, &["format", "-"], source, "mix format")
}

/// Format Kotlin via `ktfmt`. ktfmt has no stdin mode; we use the same
/// temp-file dance as csharpier and php-cs-fixer. ktlint is the
/// dominant Kotlin linter but its `--format` mode emits diagnostics
/// alongside the source, so ktfmt's narrower scope is the cleaner fit.
fn run_ktfmt(path: &Path, source: &str) -> Result<String, String> {
    let ktfmt = find_on_path("ktfmt").ok_or_else(|| {
        "ktfmt not found — install with `brew install ktfmt` or grab the jar from GitHub"
            .to_string()
    })?;
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("kt");
    let ((), text) = format_via_temp_file(path, ext, source, |temp| {
        run_tool_on_file(&ktfmt, &[], temp, "ktfmt")
    })?;
    Ok(text)
}

/// Format SQL via `sql-formatter` (the npm tool). Reads stdin, writes
/// stdout. SQL is a multi-dialect mess and the right formatter varies
/// by team; `sql-formatter` is just the most broadly applicable
/// default. Users who want pgFormatter / sleek can swap by editing
/// `format.rs`.
fn run_sql_formatter(source: &str) -> Result<String, String> {
    let bin = find_on_path("sql-formatter").ok_or_else(|| {
        "sql-formatter not found — install with `npm i -g sql-formatter`".to_string()
    })?;
    run_stdin_pipe(&bin, &[], source, "sql-formatter")
}

/// Format PHP via `php-cs-fixer`. The tool has no stdin mode and
/// always edits files in place, so we use the same temp-file dance as
/// csharpier: write the buffer next to the real file so project-level
/// `.php-cs-fixer.dist.php` config resolves, run the fixer against the
/// temp file, read the result back, unlink.
fn run_php_cs_fixer(path: &Path, source: &str) -> Result<String, String> {
    let bin = find_on_path("php-cs-fixer")
        .ok_or_else(|| "php-cs-fixer not found — install with `composer global require friendsofphp/php-cs-fixer`".to_string())?;
    let ((), text) = format_via_temp_file(path, "php", source, |temp| {
        run_tool_on_file(
            &bin,
            &["fix", "--quiet", "--using-cache=no"],
            temp,
            "php-cs-fixer",
        )
    })?;
    Ok(text)
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
        // file path on the first line. Strip that chrome first, then clip
        // like every other tool (six lines — biome's payload is wordier).
        let stripped: String = stderr
            .lines()
            .map(|l| {
                l.trim_matches(|c: char| {
                    c.is_whitespace() || matches!(c, '━' | '│' | '╭' | '╮' | '╯' | '╰' | '┃')
                })
            })
            .collect::<Vec<_>>()
            .join("\n");
        let msg = crate::package::clip_lines(stripped.as_bytes(), 6)
            .unwrap_or_else(|| "(no error output)".to_string());
        let code = output
            .status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "?".into());
        return Err(format!("biome exit {code}: {msg}"));
    }

    String::from_utf8(output.stdout).map_err(|e| format!("biome stdout not utf-8: {e}"))
}

/// Resolve the csharpier binary. Checks `$PATH` first (covers Homebrew /
/// custom installs), then falls back to the conventional dotnet global-tool
/// location at `~/.dotnet/tools/csharpier` (where `dotnet tool install -g
/// csharpier` lands by default).
fn find_csharpier() -> Option<PathBuf> {
    if let Some(p) = find_on_path("csharpier") {
        return Some(p);
    }
    let dotnet_tools = crate::paths::home_join(".dotnet/tools/csharpier")?;
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
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("cs");
    let (outcome, text) = format_via_temp_file(path, ext, source, |temp| {
        csharpier_format_inplace(&csharpier, temp)
    })?;
    match outcome {
        CsharpierOutcome::Formatted => Ok(Some(text)),
        CsharpierOutcome::Unsupported => Ok(None),
    }
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
    run_stdin_pipe(&bin, &[], source, label)
}

/// What `format_buffer` would do for `path`'s extension: which tool
/// it would invoke and whether that tool resolves on the local
/// machine. Surfaced by `:health` so the user can see, at a glance,
/// whether their on-save formatter is set up — and where the binary
/// would come from (project `node_modules` vs system `$PATH`).
pub struct FormatterStatus {
    pub label: String,
    pub binary: Option<PathBuf>,
    pub via_node_modules: bool,
    /// Optional "if the primary binary is missing, also tried these"
    /// hint — used by Python (ruff → black) and Nix (nixfmt → alejandra).
    pub fallback_label: Option<String>,
}

/// Resolve the formatter that would run on save for `path`'s
/// extension. Returns `None` when no formatter is configured.
/// Resolution mirrors what the corresponding `run_*` function does at
/// save time (project-local `node_modules/.bin` first for biome /
/// prettier, fallback chains preserved for Python and Nix).
pub fn primary_formatter_for_path(path: &Path) -> Option<FormatterStatus> {
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
    let start = path.parent().unwrap_or(Path::new("."));
    let on_path = |name: &str| FormatterStatus {
        label: name.to_string(),
        binary: find_on_path(name),
        via_node_modules: false,
        fallback_label: None,
    };
    let node_or_path = |name: &str| {
        if let Some(p) = find_node_modules_bin(start, name) {
            return FormatterStatus {
                label: name.to_string(),
                binary: Some(PathBuf::from(p)),
                via_node_modules: true,
                fallback_label: None,
            };
        }
        on_path(name)
    };
    let status = match ext {
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "json" | "jsonc" => node_or_path("biome"),
        "cs" | "cshtml" | "razor" => on_path("csharpier"),
        "go" => on_path("gofmt"),
        "py" | "pyi" => {
            let ruff = on_path("ruff");
            if ruff.binary.is_some() {
                ruff
            } else {
                let black = on_path("black");
                if black.binary.is_some() {
                    return Some(FormatterStatus {
                        label: "black".into(),
                        binary: black.binary,
                        via_node_modules: false,
                        fallback_label: None,
                    });
                }
                FormatterStatus {
                    label: "ruff".into(),
                    binary: None,
                    via_node_modules: false,
                    fallback_label: Some("black".into()),
                }
            }
        }
        "c" | "h" | "cc" | "cpp" | "cxx" | "hh" | "hpp" | "hxx" | "c++" | "h++" => {
            on_path("clang-format")
        }
        "sh" | "bash" | "zsh" | "ksh" => on_path("shfmt"),
        "lua" => on_path("stylua"),
        "md" | "markdown" | "mdx" | "vue" | "svelte" | "html" | "htm" | "css" | "scss" | "less"
        | "yaml" | "yml" | "graphql" | "gql" => node_or_path("prettier"),
        "toml" => on_path("taplo"),
        "rb" | "rake" | "gemspec" => on_path("rufo"),
        "php" => on_path("php-cs-fixer"),
        "java" => on_path("google-java-format"),
        "zig" => on_path("zig"),
        "nix" => {
            let nixfmt = on_path("nixfmt");
            if nixfmt.binary.is_some() {
                nixfmt
            } else {
                let alejandra = on_path("alejandra");
                if alejandra.binary.is_some() {
                    return Some(alejandra);
                }
                FormatterStatus {
                    label: "nixfmt".into(),
                    binary: None,
                    via_node_modules: false,
                    fallback_label: Some("alejandra".into()),
                }
            }
        }
        "ex" | "exs" => on_path("mix"),
        "kt" | "kts" => on_path("ktfmt"),
        "sql" => on_path("sql-formatter"),
        _ => return None,
    };
    Some(status)
}

/// Resolve a binary by name on `$PATH`. Delegates to the shared helper
/// in `paths` — same cross-platform PATH splitting and `.exe`/`.cmd`/
/// `.bat` fallback on Windows.
fn find_on_path(name: &str) -> Option<PathBuf> {
    crate::paths::find_on_path(name)
}

/// Shared temp-file dance for formatters that only edit files in place
/// (csharpier, ktfmt, php-cs-fixer): write the buffer to an exclusively
/// created temp beside the real file — beside it so project-level config
/// still resolves, exclusive so a symlink planted at the name fails the
/// create instead of redirecting the write — run the tool against it,
/// read the result back, unlink. `ext` is the extension the tool needs
/// to see to recognise the file type.
fn format_via_temp_file<T>(
    path: &Path,
    ext: &str,
    source: &str,
    run: impl FnOnce(&Path) -> Result<T, String>,
) -> Result<(T, String), String> {
    let (temp, mut file) =
        crate::paths::create_temp_with_ext(path, ext).map_err(|e| format!("write temp: {e}"))?;
    let written = file.write_all(source.as_bytes());
    drop(file);
    if let Err(e) = written {
        let _ = std::fs::remove_file(&temp);
        return Err(format!("write temp: {e}"));
    }
    let outcome = run(&temp).and_then(|t| {
        std::fs::read_to_string(&temp)
            .map(|text| (t, text))
            .map_err(|e| format!("read temp: {e}"))
    });
    let _ = std::fs::remove_file(&temp);
    outcome
}

/// Run an in-place formatter against `file`, mapping a non-zero exit to
/// the same `"<label> exit <code>: <lines>"` shape the stdin formatters
/// produce.
fn run_tool_on_file(bin: &Path, args: &[&str], file: &Path, label: &str) -> Result<(), String> {
    let result = Command::new(bin)
        .args(args)
        .arg(file)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();
    match result {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => {
            let msg = crate::package::clip_lines(&o.stderr, 4)
                .unwrap_or_else(|| "(no error output)".to_string());
            let code = o
                .status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "?".into());
            Err(format!("{label} exit {code}: {msg}"))
        }
        Err(e) => Err(format!("failed to spawn {label}: {e}")),
    }
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
        // stderr first, then stdout in the same clip — csharpier splits
        // its diagnostics across both.
        let combined = format!("{stderr_buf}\n{stdout_buf}");
        let msg = crate::package::clip_lines(combined.as_bytes(), 4)
            .unwrap_or_else(|| "(no error output)".to_string());
        let code = status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "?".into());
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

    #[test]
    fn empty_formatter_output_is_refused_for_a_non_empty_buffer() {
        assert!(reject_empty("fn main() {}\n", String::new()).is_err());
        assert!(reject_empty("fn main() {}\n", "\n  \n".into()).is_err());
        assert_eq!(reject_empty("", String::new()), Ok(String::new()));
        assert_eq!(reject_empty("  \n", "\n".into()), Ok("\n".into()));
        assert_eq!(
            reject_empty("a  =1", "a = 1\n".into()),
            Ok("a = 1\n".into())
        );
    }

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
        let src =
            "@{\n\tLayout = \"Master.cshtml\";\n\tvar x = 1;\n}\n<div>\n\t<a>hi</a>\n</div>\n";
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
