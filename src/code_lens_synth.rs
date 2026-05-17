//! Client-side code-lens synthesis for languages whose LSP servers
//! don't emit runnable lenses out of the box. Today that means
//! JS/TS/TSX vitest tests — tsserver has no concept of test runners,
//! so we walk the tree-sitter tree for `it("...", ...)`,
//! `test("...", ...)`, and `describe("...", ...)` invocations and
//! emit synthetic `CodeLensItem`s anchored on the call line. The
//! resulting lens is paired with a `binvim.runTestByName` command
//! that `app/lsp_glue.rs::invoke_lens_command` recognises and routes
//! into the integrated test runner — same destination as the
//! rust-analyzer `runSingle` interception.
//!
//! Not a plugin system. Adding a new language means adding an arm to
//! `synthesize_lenses` and a tree-sitter walk for that language's
//! test framework. The renderer + execution path are unchanged.

use tree_sitter::{Node, Parser};

use crate::buffer::Buffer;
use crate::lang::Lang;
use crate::lsp::{CodeLensItem, LspCommand};

/// Command name our synthetic lenses carry. Distinct from any
/// `workspace/executeCommand` the server side knows about so the
/// fallback path (sending the command back to the LSP) doesn't fire
/// for synthetic items — they're terminal at the binvim side.
pub const SYNTHETIC_RUN_COMMAND: &str = "binvim.runTestByName";

/// Emit synthetic code lenses for `buf` if the language is one we
/// know how to parse for tests. Returns an empty vec for unsupported
/// languages or when the parse fails — callers merge unconditionally.
pub fn synthesize_lenses(lang: Lang, buf: &Buffer) -> Vec<CodeLensItem> {
    match lang {
        Lang::TypeScript | Lang::Tsx | Lang::JavaScript => synthesize_js_ts(lang, buf),
        _ => Vec::new(),
    }
}

fn synthesize_js_ts(lang: Lang, buf: &Buffer) -> Vec<CodeLensItem> {
    let language = lang.ts_language();
    let mut parser = Parser::new();
    if parser.set_language(&language).is_err() {
        return Vec::new();
    }
    let source = buf.rope.to_string();
    let Some(tree) = parser.parse(&source, None) else {
        return Vec::new();
    };
    let src = source.as_bytes();
    let mut out = Vec::new();
    let mut seen: std::collections::HashSet<(usize, String)> =
        std::collections::HashSet::new();
    walk_for_test_calls(tree.root_node(), src, &mut out, &mut seen);
    out
}

fn walk_for_test_calls(
    node: Node,
    src: &[u8],
    out: &mut Vec<CodeLensItem>,
    seen: &mut std::collections::HashSet<(usize, String)>,
) {
    if node.kind() == "call_expression" {
        if let Some(item) = maybe_test_call(node, src) {
            // Dedupe by (line, name) — the same line could appear in
            // a parent walk and a child walk during recursion if
            // tree-sitter ever changes its node structure. Cheap
            // insurance.
            let key = (item.line, lens_name_arg(&item).unwrap_or_default());
            if seen.insert(key) {
                out.push(item);
            }
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_for_test_calls(child, src, out, seen);
    }
}

/// Match `it("...", …)`, `it.skip("...", …)`, `test.only("...", …)`,
/// `describe("...", …)`, etc. Returns the synthetic lens anchored on
/// the call's start line. The kind (`it` vs `describe`) drives the
/// rendered title; the test name goes into the command arguments.
fn maybe_test_call(call: Node, src: &[u8]) -> Option<CodeLensItem> {
    let fn_node = call.child_by_field_name("function")?;
    let kind = match fn_node.kind() {
        "identifier" => identifier_text(fn_node, src)?.to_string(),
        "member_expression" => {
            // `it.skip` / `test.only` / `describe.each` etc. — the
            // root identifier is what we care about. Walk down the
            // object chain to the leftmost identifier.
            let obj = fn_node.child_by_field_name("object")?;
            leftmost_identifier(obj, src)?.to_string()
        }
        _ => return None,
    };
    if !matches!(kind.as_str(), "it" | "test" | "describe") {
        return None;
    }
    let args = call.child_by_field_name("arguments")?;
    // Walk arg children, find the first string literal — skips a
    // potential `template_string` first arg (we don't try to
    // interpolate templates) and similar.
    let name = first_string_literal_text(args, src)?;
    if name.is_empty() {
        return None;
    }
    let start = call.start_position();
    let line = start.row;
    let col = start.column;
    let label = if kind == "describe" { "Run Suite" } else { "Run Test" };
    let command = LspCommand {
        title: format!("▶ {label}"),
        command: SYNTHETIC_RUN_COMMAND.to_string(),
        arguments: vec![serde_json::json!({
            "name": name,
            "kind": kind,
        })],
    };
    Some(CodeLensItem {
        line,
        col,
        command: Some(command),
    })
}

/// Pull the name out of a synthetic-lens `CodeLensItem` for dedupe.
/// Synthetic items always carry a `name` in the first argument
/// object; LSP items can take any shape so this returns None for
/// anything that doesn't match.
fn lens_name_arg(item: &CodeLensItem) -> Option<String> {
    let cmd = item.command.as_ref()?;
    let arg = cmd.arguments.first()?;
    arg.get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn identifier_text<'a>(node: Node, src: &'a [u8]) -> Option<&'a str> {
    std::str::from_utf8(&src[node.start_byte()..node.end_byte()]).ok()
}

fn leftmost_identifier<'a>(node: Node, src: &'a [u8]) -> Option<&'a str> {
    let mut cur = node;
    loop {
        match cur.kind() {
            "identifier" => return identifier_text(cur, src),
            "member_expression" => {
                cur = cur.child_by_field_name("object")?;
            }
            _ => return None,
        }
    }
}

fn first_string_literal_text(args: Node, src: &[u8]) -> Option<String> {
    let mut cursor = args.walk();
    for child in args.children(&mut cursor) {
        match child.kind() {
            "string" => return decode_string_literal(child, src),
            // `template_string` without substitutions can still
            // resolve to a stable name (e.g. `it(`when X happens`, …)`).
            // Only accept it when it has no `template_substitution`
            // children — we don't try to interpret interpolation.
            "template_string" => {
                if !has_template_substitution(child) {
                    return decode_template_string(child, src);
                }
            }
            _ => {}
        }
    }
    None
}

fn decode_string_literal(node: Node, src: &[u8]) -> Option<String> {
    // tree-sitter-typescript exposes string contents as
    // `string_fragment` children; concatenating them yields the
    // unescaped-ish text. We don't bother resolving backslash
    // escapes here — the test name is for filter substring matching
    // and vitest's `-t` is a literal substring, so a `\n` in the
    // name is the user's problem anyway.
    let mut cursor = node.walk();
    let mut out = String::new();
    for child in node.children(&mut cursor) {
        if child.kind() == "string_fragment" {
            out.push_str(std::str::from_utf8(&src[child.start_byte()..child.end_byte()]).ok()?);
        }
    }
    if out.is_empty() {
        // Empty string literal (`""`) — return Some("") so the caller
        // can still skip it via the `is_empty` check; treating it as
        // None would conflate "no string here" with "empty string."
        return Some(String::new());
    }
    Some(out)
}

fn decode_template_string(node: Node, src: &[u8]) -> Option<String> {
    // No-substitution templates only — tree-sitter exposes the
    // contents as a `string_fragment` child. Trim the backticks at
    // the boundaries.
    let mut cursor = node.walk();
    let mut out = String::new();
    for child in node.children(&mut cursor) {
        if child.kind() == "string_fragment" {
            out.push_str(std::str::from_utf8(&src[child.start_byte()..child.end_byte()]).ok()?);
        }
    }
    Some(out)
}

fn has_template_substitution(node: Node) -> bool {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "template_substitution" {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffer::Buffer;

    fn buf(src: &str) -> Buffer {
        let mut b = Buffer::empty();
        b.rope = ropey::Rope::from_str(src);
        b
    }

    #[test]
    fn it_call_emits_lens_with_test_name() {
        let src = "import { it } from 'vitest';\nit(\"adds numbers\", () => {});\n";
        let lenses = synthesize_lenses(Lang::TypeScript, &buf(src));
        assert_eq!(lenses.len(), 1);
        let lens = &lenses[0];
        assert_eq!(lens.line, 1);
        let cmd = lens.command.as_ref().unwrap();
        assert_eq!(cmd.command, SYNTHETIC_RUN_COMMAND);
        assert_eq!(cmd.arguments[0]["name"], "adds numbers");
        assert_eq!(cmd.arguments[0]["kind"], "it");
    }

    #[test]
    fn test_describe_and_skip_chain_all_recognised() {
        let src = "\
describe(\"suite\", () => {
  it(\"one\", () => {});
  it.skip(\"two\", () => {});
  test.only(\"three\", () => {});
});
";
        let lenses = synthesize_lenses(Lang::TypeScript, &buf(src));
        let names: Vec<String> = lenses
            .iter()
            .filter_map(|l| l.command.as_ref())
            .map(|c| c.arguments[0]["name"].as_str().unwrap_or("").to_string())
            .collect();
        assert!(names.contains(&"suite".to_string()));
        assert!(names.contains(&"one".to_string()));
        assert!(names.contains(&"two".to_string()));
        assert!(names.contains(&"three".to_string()));
        assert_eq!(lenses.len(), 4);
    }

    #[test]
    fn tsx_files_get_synthetic_lenses_too() {
        let src = "it('renders', () => {});\n";
        let lenses = synthesize_lenses(Lang::Tsx, &buf(src));
        assert_eq!(lenses.len(), 1);
        assert_eq!(
            lenses[0].command.as_ref().unwrap().arguments[0]["name"],
            "renders"
        );
    }

    #[test]
    fn template_literal_without_interpolation_is_extracted() {
        let src = "it(`stable name`, () => {});\n";
        let lenses = synthesize_lenses(Lang::TypeScript, &buf(src));
        assert_eq!(lenses.len(), 1);
        assert_eq!(
            lenses[0].command.as_ref().unwrap().arguments[0]["name"],
            "stable name"
        );
    }

    #[test]
    fn template_literal_with_interpolation_is_skipped() {
        let src = "const x = 1; it(`name ${x}`, () => {});\n";
        let lenses = synthesize_lenses(Lang::TypeScript, &buf(src));
        assert!(lenses.is_empty(), "interpolated templates skipped");
    }

    #[test]
    fn unsupported_language_returns_empty_vec() {
        let src = "fn foo() {}";
        assert!(synthesize_lenses(Lang::Rust, &buf(src)).is_empty());
    }

    #[test]
    fn unrelated_call_expressions_are_ignored() {
        let src = "expect(foo).toBe(bar);\nconsole.log('hi');\n";
        assert!(synthesize_lenses(Lang::TypeScript, &buf(src)).is_empty());
    }

    #[test]
    fn javascript_dialect_is_recognised() {
        let src = "it('plain js', () => {});\n";
        let lenses = synthesize_lenses(Lang::JavaScript, &buf(src));
        assert_eq!(lenses.len(), 1);
    }
}
