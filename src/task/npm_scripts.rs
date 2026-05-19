//! `package.json:scripts` discovery — npm / pnpm / yarn aware.
//!
//! Picks the package manager by lockfile (pnpm-lock.yaml → pnpm,
//! yarn.lock → yarn, otherwise npm). Doesn't shell out to `npm pkg get
//! scripts` — that adds a 200-500ms hit per discovery call on slow
//! machines and the JSON is trivial to scan manually.

use std::path::Path;

use super::types::{Task, TaskSource};

/// Lockfile basenames that mark a workspace this adapter claims. Listed
/// in resolution order — the first one we hit while walking up wins,
/// which is what `find_root` returns.
pub const ROOT_MARKERS: &[&str] = &["package.json"];

/// Scan a directory's `package.json` for a `"scripts"` object and emit
/// one `Task` per entry. Quietly returns an empty Vec on any parse
/// failure — discovery is best-effort, never blocking.
pub fn discover(root: &Path) -> Vec<Task> {
    let pkg_path = root.join("package.json");
    let Ok(contents) = std::fs::read_to_string(&pkg_path) else {
        return Vec::new();
    };
    let scripts = match extract_scripts(&contents) {
        Some(s) => s,
        None => return Vec::new(),
    };
    let pm = detect_package_manager(root);
    scripts
        .into_iter()
        .map(|(name, body)| Task {
            label: name.clone(),
            source: TaskSource::NpmScripts,
            cwd: root.to_path_buf(),
            program: pm.program().to_string(),
            args: pm.args_for_script(&name),
            description: Some(body),
        })
        .collect()
}

/// Package-manager flavours we know how to invoke. Detection is purely
/// lockfile-based; we don't read `engines.packageManager` or walk
/// `.npmrc` since the lockfile is the authoritative signal in every
/// project that has one.
#[derive(Debug, Clone, Copy)]
enum PackageManager {
    Pnpm,
    Yarn,
    Npm,
}

impl PackageManager {
    fn program(self) -> &'static str {
        match self {
            PackageManager::Pnpm => "pnpm",
            PackageManager::Yarn => "yarn",
            PackageManager::Npm => "npm",
        }
    }

    /// CLI arg list to invoke `script` under this package manager.
    /// pnpm / yarn run a script by name directly; npm uses the
    /// `run-script` verb (or the `run` alias, but we prefer the
    /// canonical form so output formatting matches what's documented).
    fn args_for_script(self, script: &str) -> Vec<String> {
        match self {
            PackageManager::Pnpm | PackageManager::Yarn => vec![script.to_string()],
            PackageManager::Npm => vec!["run".to_string(), script.to_string()],
        }
    }
}

fn detect_package_manager(root: &Path) -> PackageManager {
    if root.join("pnpm-lock.yaml").exists() {
        PackageManager::Pnpm
    } else if root.join("yarn.lock").exists() {
        PackageManager::Yarn
    } else {
        PackageManager::Npm
    }
}

/// Pull the `scripts` object out of a `package.json` blob. Hand-rolled
/// rather than pulling in serde_json — the file format is stable, the
/// scan is dominated by I/O cost, and we only care about a single
/// well-known top-level object. Returns `(name, body)` pairs in source
/// order so the picker presents scripts the way the author listed them.
fn extract_scripts(json: &str) -> Option<Vec<(String, String)>> {
    let scripts_start = find_top_level_key(json, "scripts")?;
    // After the key + colon we expect `{` opening the scripts object.
    let after_key = &json[scripts_start..];
    let obj_start = after_key.find('{')?;
    let obj_body = &after_key[obj_start + 1..];
    // Track brace depth so a `}` inside a script value doesn't end the
    // object prematurely. (Unlikely but possible — `"foo": "echo }"`.)
    let mut depth = 1usize;
    let mut end = 0usize;
    let mut in_str = false;
    let mut esc = false;
    for (i, b) in obj_body.bytes().enumerate() {
        if in_str {
            if esc {
                esc = false;
            } else if b == b'\\' {
                esc = true;
            } else if b == b'"' {
                in_str = false;
            }
            continue;
        }
        match b {
            b'"' => in_str = true,
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    end = i;
                    break;
                }
            }
            _ => {}
        }
    }
    if depth != 0 {
        return None;
    }
    let body = &obj_body[..end];
    let mut out = Vec::new();
    parse_object_entries(body, &mut out);
    Some(out)
}

/// Walk a JSON object body (the chars between `{` and `}`) and pull
/// out `(key, value)` pairs where both are string literals. Skips
/// non-string values silently — the discoverer only cares about
/// scripts that are strings (the standard form).
fn parse_object_entries(body: &str, out: &mut Vec<(String, String)>) {
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Skip whitespace + commas.
        while i < bytes.len() && (bytes[i] as char).is_whitespace()
            || (i < bytes.len() && bytes[i] == b',')
        {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        // Expect an opening `"` for the key.
        if bytes[i] != b'"' {
            // Malformed — try to skip to the next `,` and continue.
            while i < bytes.len() && bytes[i] != b',' {
                i += 1;
            }
            continue;
        }
        let (key, after_key) = match read_json_string(&bytes[i..]) {
            Some(pair) => pair,
            None => break,
        };
        i += after_key;
        // Skip whitespace + the colon.
        while i < bytes.len() && (bytes[i] as char).is_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b':' {
            continue;
        }
        i += 1;
        while i < bytes.len() && (bytes[i] as char).is_whitespace() {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        if bytes[i] != b'"' {
            // Non-string value — skip the value. Cheap heuristic: walk
            // until the next top-level `,` or end.
            let mut depth = 0i32;
            while i < bytes.len() {
                match bytes[i] {
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => depth -= 1,
                    b',' if depth == 0 => break,
                    _ => {}
                }
                i += 1;
            }
            continue;
        }
        let (value, after_val) = match read_json_string(&bytes[i..]) {
            Some(pair) => pair,
            None => break,
        };
        i += after_val;
        out.push((key, value));
    }
}

/// Read a JSON string starting at the leading `"`. Returns the unquoted
/// contents and the byte offset just past the closing `"`. Handles the
/// escape sequences npm scripts actually use (`\"`, `\\`, `\n`, `\t`);
/// unknown escapes pass through as the literal escaped char so we don't
/// reject valid but exotic content.
fn read_json_string(bytes: &[u8]) -> Option<(String, usize)> {
    if bytes.is_empty() || bytes[0] != b'"' {
        return None;
    }
    let mut out = String::new();
    let mut i = 1;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'"' {
            return Some((out, i + 1));
        }
        if b == b'\\' && i + 1 < bytes.len() {
            let n = bytes[i + 1];
            match n {
                b'"' => out.push('"'),
                b'\\' => out.push('\\'),
                b'/' => out.push('/'),
                b'n' => out.push('\n'),
                b'r' => out.push('\r'),
                b't' => out.push('\t'),
                other => out.push(other as char),
            }
            i += 2;
            continue;
        }
        out.push(b as char);
        i += 1;
    }
    None
}

/// Find a top-level key string in a JSON blob. Returns the byte offset
/// just past the closing quote of the key, or `None` if the key isn't
/// at top level (e.g. a `"scripts"` nested inside another object
/// shouldn't match — we want the file's outermost one).
fn find_top_level_key(json: &str, key: &str) -> Option<usize> {
    let bytes = json.as_bytes();
    let mut depth = 0i32;
    let mut in_str = false;
    let mut esc = false;
    let mut i = 0;
    let needle = format!("\"{}\"", key);
    let needle_bytes = needle.as_bytes();
    while i < bytes.len() {
        let b = bytes[i];
        if in_str {
            if esc {
                esc = false;
            } else if b == b'\\' {
                esc = true;
            } else if b == b'"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        match b {
            b'"' => {
                // Only top-level keys count — `scripts` nested inside
                // another object would land at depth > 1 (the outer
                // `{` puts us at depth 1).
                if depth == 1 && bytes[i..].starts_with(needle_bytes) {
                    return Some(i + needle_bytes.len());
                }
                in_str = true;
            }
            b'{' | b'[' => depth += 1,
            b'}' | b']' => depth -= 1,
            _ => {}
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_scripts_pulls_three() {
        let json = r#"{
  "name": "x",
  "scripts": {
    "build": "tsc -p .",
    "dev": "vite dev",
    "lint": "eslint ."
  }
}"#;
        let scripts = extract_scripts(json).unwrap();
        let names: Vec<&str> = scripts.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(names, vec!["build", "dev", "lint"]);
        assert_eq!(scripts[0].1, "tsc -p .");
    }

    #[test]
    fn extract_scripts_returns_none_when_missing() {
        let json = r#"{"name":"x"}"#;
        assert!(extract_scripts(json).is_none());
    }

    #[test]
    fn detect_package_manager_via_lockfile() {
        let tmp = std::env::temp_dir().join("binvim-task-pm");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        assert!(matches!(detect_package_manager(&tmp), PackageManager::Npm));
        std::fs::write(tmp.join("pnpm-lock.yaml"), "").unwrap();
        assert!(matches!(detect_package_manager(&tmp), PackageManager::Pnpm));
        std::fs::remove_file(tmp.join("pnpm-lock.yaml")).unwrap();
        std::fs::write(tmp.join("yarn.lock"), "").unwrap();
        assert!(matches!(detect_package_manager(&tmp), PackageManager::Yarn));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn discover_emits_npm_run_args() {
        let tmp = std::env::temp_dir().join("binvim-task-npm-discover");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(
            tmp.join("package.json"),
            r#"{"scripts":{"build":"tsc","dev":"vite"}}"#,
        )
        .unwrap();
        let tasks = discover(&tmp);
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].label, "build");
        assert_eq!(tasks[0].program, "npm");
        assert_eq!(tasks[0].args, vec!["run", "build"]);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn discover_uses_pnpm_when_lockfile_present() {
        let tmp = std::env::temp_dir().join("binvim-task-pnpm-discover");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("package.json"), r#"{"scripts":{"dev":"vite"}}"#).unwrap();
        std::fs::write(tmp.join("pnpm-lock.yaml"), "").unwrap();
        let tasks = discover(&tmp);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].program, "pnpm");
        assert_eq!(tasks[0].args, vec!["dev"]);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
