//! User key mappings — the `[keymaps]` config section.
//!
//! A mapping replaces one key with a sequence of keys, the way Vim's
//! `nnoremap` / `vnoremap` do. The sequence is fed back through the normal
//! input path, so it reaches anything a typed key can — motions, operators,
//! leader chords, `:` commands — without the config naming `Action` variants,
//! which would freeze their names into a public format.
//!
//! Where a mapping may fire is the parser's call, not this module's: see
//! `PendingCmd::accepts_mapping`.
//!
//! ```toml
//! [keymaps.normal]
//! H = "^"
//! L = "$"
//! J = "10j"
//! "<C-s>" = ":w<CR>"
//!
//! [keymaps.visual]
//! H = "^"
//! ```

use crate::parser::ParseCtx;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::Deserialize;
use std::collections::HashMap;

type KeyId = (KeyCode, KeyModifiers);

#[derive(Debug, Default)]
pub struct Keymaps {
    normal: HashMap<KeyId, Vec<KeyEvent>>,
    visual: HashMap<KeyId, Vec<KeyEvent>>,
    /// Entries that didn't parse, one line each. They're skipped rather than
    /// failing the load: `Config::load` falls back to the default config on
    /// any deserialize error, so one typo'd mapping would take the user's
    /// colours and every other setting down with it.
    pub errors: Vec<String>,
}

impl Keymaps {
    /// The keys `key` expands to in `ctx`. An empty slice is a `<Nop>`
    /// mapping — the key is switched off.
    pub fn get(&self, ctx: ParseCtx, key: &KeyEvent) -> Option<&[KeyEvent]> {
        let table = match ctx {
            ParseCtx::Normal => &self.normal,
            ParseCtx::Visual => &self.visual,
        };
        table.get(&key_id(key)).map(Vec::as_slice)
    }
}

impl<'de> Deserialize<'de> for Keymaps {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default)]
            normal: HashMap<String, String>,
            #[serde(default)]
            visual: HashMap<String, String>,
        }
        let raw = Raw::deserialize(d)?;
        let mut errors = Vec::new();
        let normal = compile_table("normal", raw.normal, &mut errors);
        let visual = compile_table("visual", raw.visual, &mut errors);
        // The tables iterate in HashMap order; sort so the startup notice
        // names the same entry on every launch.
        errors.sort();
        Ok(Self {
            normal,
            visual,
            errors,
        })
    }
}

fn compile_table(
    mode: &str,
    entries: HashMap<String, String>,
    errors: &mut Vec<String>,
) -> HashMap<KeyId, Vec<KeyEvent>> {
    let mut table = HashMap::new();
    for (lhs, rhs) in entries {
        match compile(&lhs, &rhs) {
            Ok((id, keys)) => {
                table.insert(id, keys);
            }
            Err(e) => errors.push(format!("[keymaps.{mode}] {lhs}: {e}")),
        }
    }
    table
}

fn compile(lhs: &str, rhs: &str) -> Result<(KeyId, Vec<KeyEvent>), String> {
    let &[from] = parse_keys(lhs)?.as_slice() else {
        return Err("maps a single key — multi-key mappings aren't supported yet".into());
    };
    if rhs.eq_ignore_ascii_case("<nop>") {
        return Ok((key_id(&from), Vec::new()));
    }
    let to = parse_keys(rhs)?;
    if to.is_empty() {
        return Err(r#"empty — map it to "<Nop>" to switch the key off"#.into());
    }
    Ok((key_id(&from), to))
}

/// What a key is matched on. Terminals disagree on whether an uppercase
/// letter or BackTab also carries SHIFT, and Ctrl-letters arrive in either
/// case — the parser treats `<C-r>` and `<C-R>` alike, so the table does too.
fn key_id(k: &KeyEvent) -> KeyId {
    match k.code {
        KeyCode::Char(c) => {
            let mods = k.modifiers.difference(KeyModifiers::SHIFT);
            let c = if mods.contains(KeyModifiers::CONTROL) {
                c.to_ascii_lowercase()
            } else {
                c
            };
            (KeyCode::Char(c), mods)
        }
        KeyCode::BackTab => (
            KeyCode::BackTab,
            k.modifiers.difference(KeyModifiers::SHIFT),
        ),
        code => (code, k.modifiers),
    }
}

/// Vim key notation: plain characters stand for themselves, `<…>` names a
/// special key or a modified one (`<Space>`, `<CR>`, `<C-r>`, `<S-Tab>`,
/// `<leader>`). A `<` that doesn't open key notation is a literal `<`, so
/// `<<` stays the shift-left operator; `<lt>` spells one explicitly.
fn parse_keys(s: &str) -> Result<Vec<KeyEvent>, String> {
    let mut keys = Vec::new();
    let mut rest = s;
    while let Some(c) = rest.chars().next() {
        if c == '<'
            && let Some(end) = rest.find('>')
            && let Some(named) = parse_named(&rest[1..end])
        {
            keys.push(named?);
            rest = &rest[end + 1..];
            continue;
        }
        keys.push(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        rest = &rest[c.len_utf8()..];
    }
    Ok(keys)
}

/// `None` when `name` isn't key notation at all and the `<` is literal;
/// `Some(Err)` when it is notation but names no key binvim knows, so a typo
/// like `<Spce>` is reported instead of being typed out character by
/// character.
fn parse_named(name: &str) -> Option<Result<KeyEvent, String>> {
    let mut mods = KeyModifiers::NONE;
    let mut base = name;
    // A modifier is a letter and a dash with something after it, so `<C-->`
    // is Ctrl + `-`.
    while base.len() > 2 && base.as_bytes()[1] == b'-' {
        mods |= match base.as_bytes()[0].to_ascii_uppercase() {
            b'C' => KeyModifiers::CONTROL,
            b'A' | b'M' => KeyModifiers::ALT,
            b'D' => KeyModifiers::SUPER,
            b'S' => KeyModifiers::SHIFT,
            _ => break,
        };
        base = &base[2..];
    }
    let mut chars = base.chars();
    let code = match (chars.next(), chars.next()) {
        (None, _) => return None,
        // `<x>` is three literal keys, as in Vim.
        (Some(_), None) if mods.is_empty() => return None,
        (Some(c), None) => KeyCode::Char(c),
        _ if !base.chars().all(|c| c.is_ascii_alphanumeric()) => return None,
        _ => match base.to_ascii_lowercase().as_str() {
            "space" | "leader" => KeyCode::Char(' '),
            "cr" | "enter" | "return" => KeyCode::Enter,
            "esc" => KeyCode::Esc,
            "tab" => KeyCode::Tab,
            "bs" | "backspace" => KeyCode::Backspace,
            "del" | "delete" => KeyCode::Delete,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" | "pgup" => KeyCode::PageUp,
            "pagedown" | "pgdn" => KeyCode::PageDown,
            "ins" | "insert" => KeyCode::Insert,
            "lt" => KeyCode::Char('<'),
            "bar" => KeyCode::Char('|'),
            "bslash" => KeyCode::Char('\\'),
            other => match other.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
                Some(n @ 1..=24) => KeyCode::F(n),
                _ => return Some(Err(format!("unknown key <{name}>"))),
            },
        },
    };
    // Fold Shift into the key the way terminals report it.
    let shift = mods.contains(KeyModifiers::SHIFT);
    let (code, mods) = match code {
        KeyCode::Char(c) if shift => (
            KeyCode::Char(c.to_ascii_uppercase()),
            mods.difference(KeyModifiers::SHIFT),
        ),
        KeyCode::Tab if shift => (KeyCode::BackTab, mods.difference(KeyModifiers::SHIFT)),
        _ => (code, mods),
    };
    Some(Ok(KeyEvent::new(code, mods)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn codes(s: &str) -> Vec<(KeyCode, KeyModifiers)> {
        parse_keys(s)
            .expect("parses")
            .iter()
            .map(|k| (k.code, k.modifiers))
            .collect()
    }

    #[test]
    fn plain_characters_are_their_own_keys() {
        assert_eq!(
            codes("10j"),
            vec![
                (KeyCode::Char('1'), KeyModifiers::NONE),
                (KeyCode::Char('0'), KeyModifiers::NONE),
                (KeyCode::Char('j'), KeyModifiers::NONE),
            ]
        );
    }

    #[test]
    fn named_keys_parse() {
        let none = KeyModifiers::NONE;
        assert_eq!(codes("<Space>")[0], (KeyCode::Char(' '), none));
        assert_eq!(codes("<leader>")[0], (KeyCode::Char(' '), none));
        assert_eq!(codes("<cr>")[0], (KeyCode::Enter, none));
        assert_eq!(codes("<Esc>")[0], (KeyCode::Esc, none));
        assert_eq!(codes("<lt>")[0], (KeyCode::Char('<'), none));
        assert_eq!(codes("<F5>")[0], (KeyCode::F(5), none));
        assert_eq!(codes("<S-Tab>")[0], (KeyCode::BackTab, none));
        assert_eq!(codes("<S-a>")[0], (KeyCode::Char('A'), none));
        assert_eq!(
            codes("<C-r>")[0],
            (KeyCode::Char('r'), KeyModifiers::CONTROL)
        );
        assert_eq!(
            codes("<C-->")[0],
            (KeyCode::Char('-'), KeyModifiers::CONTROL)
        );
        assert_eq!(codes("<M-x>")[0], (KeyCode::Char('x'), KeyModifiers::ALT));
    }

    #[test]
    fn notation_mixes_with_plain_characters() {
        assert_eq!(
            codes(":w<CR>"),
            vec![
                (KeyCode::Char(':'), KeyModifiers::NONE),
                (KeyCode::Char('w'), KeyModifiers::NONE),
                (KeyCode::Enter, KeyModifiers::NONE),
            ]
        );
    }

    #[test]
    fn a_stray_angle_bracket_is_literal() {
        assert_eq!(codes("<<").len(), 2);
        assert_eq!(codes("<j>").len(), 3);
        assert_eq!(codes("<>").len(), 2);
    }

    #[test]
    fn unknown_notation_is_an_error() {
        let err = parse_keys("<Spce>").unwrap_err();
        assert!(err.contains("<Spce>"), "{err}");
        assert!(parse_keys("<F99>").is_err());
    }

    #[test]
    fn config_compiles_good_entries_and_reports_bad_ones() {
        let keymaps: Keymaps = toml::from_str(
            r#"
            [normal]
            H = "^"
            J = "10j"
            jk = "x"
            X = "<Spce>"
            Z = ""

            [visual]
            H = "^"
            "#,
        )
        .expect("a bad entry must not fail the whole table");
        assert_eq!(
            keymaps.get(ParseCtx::Normal, &plain('H')),
            Some(&[plain('^')][..])
        );
        assert_eq!(
            keymaps.get(ParseCtx::Normal, &plain('J')).map(<[_]>::len),
            Some(3)
        );
        assert!(keymaps.get(ParseCtx::Visual, &plain('H')).is_some());
        assert!(keymaps.get(ParseCtx::Visual, &plain('J')).is_none());
        assert_eq!(keymaps.errors.len(), 3, "{:?}", keymaps.errors);
        assert!(keymaps.errors[0].starts_with("[keymaps.normal] X:"));
    }

    #[test]
    fn nop_switches_a_key_off() {
        let keymaps: Keymaps = toml::from_str("[normal]\nH = \"<Nop>\"").unwrap();
        assert_eq!(keymaps.get(ParseCtx::Normal, &plain('H')), Some(&[][..]));
        assert!(keymaps.errors.is_empty());
    }

    #[test]
    fn lookup_ignores_how_the_terminal_reports_shift() {
        let keymaps: Keymaps = toml::from_str("[normal]\nH = \"^\"\n\"<C-r>\" = \"u\"").unwrap();
        let shifted_h = KeyEvent::new(KeyCode::Char('H'), KeyModifiers::SHIFT);
        assert!(keymaps.get(ParseCtx::Normal, &shifted_h).is_some());
        let ctrl_shift_r = KeyEvent::new(
            KeyCode::Char('R'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );
        assert!(keymaps.get(ParseCtx::Normal, &ctrl_shift_r).is_some());
    }
}
