//! User key mappings — the `[keymaps]` config section.
//!
//! A mapping replaces a key, or a sequence of keys, with another sequence,
//! the way Vim's `nnoremap` / `vnoremap` do. The sequence is fed back through
//! the normal input path, so it reaches anything a typed key can — motions,
//! operators, leader chords, `:` commands — without the config naming
//! `Action` variants, which would freeze their names into a public format.
//!
//! Where a mapping may fire is the parser's call, not this module's: see
//! `PendingCmd::accepts_mapping`. Holding a partly typed sequence and
//! resolving it on a mismatch or timeout is `App`'s, in `app/input.rs`.
//!
//! ```toml
//! [keymaps]
//! timeout = 1000
//!
//! [keymaps.normal]
//! H = "^"
//! gh = "^"
//! J = "10j"
//! "<leader>w" = ":w<CR>"
//!
//! [keymaps.visual]
//! H = "^"
//! ```

use crate::parser::ParseCtx;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::Deserialize;
use std::collections::HashMap;
use std::time::Duration;

/// The modes a `[keymaps]` table exists for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapMode {
    Normal,
    Visual,
    Insert,
}

impl MapMode {
    /// The parser context the mode's keys go through — `None` for Insert,
    /// where a key is text rather than part of a command.
    pub fn parse_ctx(self) -> Option<ParseCtx> {
        match self {
            Self::Normal => Some(ParseCtx::Normal),
            Self::Visual => Some(ParseCtx::Visual),
            Self::Insert => None,
        }
    }
}

impl From<ParseCtx> for MapMode {
    fn from(ctx: ParseCtx) -> Self {
        match ctx {
            ParseCtx::Normal => Self::Normal,
            ParseCtx::Visual => Self::Visual,
        }
    }
}

type KeyId = (KeyCode, KeyModifiers);
type Table = HashMap<Vec<KeyId>, Mapping>;

#[derive(Debug)]
struct Mapping {
    keys: Vec<KeyEvent>,
    /// Shown in the which-key popup in place of the keys.
    desc: Option<String>,
}

impl Mapping {
    fn label(&self) -> String {
        match &self.desc {
            Some(desc) => desc.clone(),
            None if self.keys.is_empty() => "<Nop>".into(),
            None => self.keys.iter().map(|k| key_label(&key_id(k))).collect(),
        }
    }
}

/// A `[keymaps]` value: the keys alone, or `{ keys = "…", desc = "…" }`.
#[derive(Deserialize)]
#[serde(untagged)]
enum RawMapping {
    Keys(String),
    Described { keys: String, desc: Option<String> },
}

/// Vim's `timeoutlen` default.
const DEFAULT_TIMEOUT_MS: u64 = 1000;

#[derive(Debug)]
pub struct Keymaps {
    normal: Table,
    visual: Table,
    insert: Table,
    /// How long a partly typed multi-key mapping waits for its next key
    /// before the held keys run as typed.
    pub timeout: Duration,
    /// Entries that didn't parse, one line each. They're skipped rather than
    /// failing the load: `Config::load` falls back to the default config on
    /// any deserialize error, so one typo'd mapping would take the user's
    /// colours and every other setting down with it.
    pub errors: Vec<String>,
}

impl Default for Keymaps {
    fn default() -> Self {
        Self {
            normal: Table::new(),
            visual: Table::new(),
            insert: Table::new(),
            timeout: Duration::from_millis(DEFAULT_TIMEOUT_MS),
            errors: Vec::new(),
        }
    }
}

/// How the keys typed so far relate to the mapping table.
pub enum KeymapMatch<'a> {
    /// No mapping starts with them.
    None,
    /// A longer mapping starts with them, so they wait for the next key.
    /// That includes keys that are a mapping of their own: with `J` and `Jk`
    /// both mapped, `J` runs only once the wait times out or the next key
    /// rules `Jk` out.
    Pending,
    Full(&'a [KeyEvent]),
}

impl Keymaps {
    pub fn lookup(&self, mode: MapMode, keys: &[KeyEvent]) -> KeymapMatch<'_> {
        let ids: Vec<KeyId> = keys.iter().map(key_id).collect();
        let table = self.table(mode);
        if table
            .keys()
            .any(|lhs| lhs.len() > ids.len() && lhs.starts_with(&ids))
        {
            return KeymapMatch::Pending;
        }
        match table.get(&ids) {
            Some(mapping) => KeymapMatch::Full(&mapping.keys),
            None => KeymapMatch::None,
        }
    }

    /// The expansion for exactly `keys`, ignoring longer mappings. An empty
    /// slice is a `<Nop>` mapping — the keys are switched off.
    pub fn exact(&self, mode: MapMode, keys: &[KeyEvent]) -> Option<&[KeyEvent]> {
        let ids: Vec<KeyId> = keys.iter().map(key_id).collect();
        self.table(mode).get(&ids).map(|m| m.keys.as_slice())
    }

    /// Layers the mappings one key past `chord` over the which-key rows for
    /// it: a mapping on a key the popup already lists replaces that row, the
    /// rest are added. Each shows its `desc`, or else the keys it types.
    pub fn merge_whichkey(
        &self,
        mode: MapMode,
        chord: &[KeyEvent],
        entries: &mut Vec<(String, String)>,
    ) {
        let chord: Vec<KeyId> = chord.iter().map(key_id).collect();
        let mut rows: Vec<(String, String)> = self
            .table(mode)
            .iter()
            .filter(|(lhs, _)| lhs.len() == chord.len() + 1 && lhs.starts_with(&chord))
            .map(|(lhs, mapping)| (key_label(&lhs[chord.len()]), mapping.label()))
            .collect();
        // The table iterates in HashMap order; sort so the added rows don't
        // reshuffle from one popup to the next.
        rows.sort();
        for (key, desc) in rows {
            match entries.iter_mut().find(|(k, _)| *k == key) {
                Some(row) => row.1 = desc,
                None => entries.push((key, desc)),
            }
        }
    }

    fn table(&self, mode: MapMode) -> &Table {
        match mode {
            MapMode::Normal => &self.normal,
            MapMode::Visual => &self.visual,
            MapMode::Insert => &self.insert,
        }
    }
}

impl<'de> Deserialize<'de> for Keymaps {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default = "default_timeout_ms")]
            timeout: u64,
            #[serde(default)]
            normal: HashMap<String, RawMapping>,
            #[serde(default)]
            visual: HashMap<String, RawMapping>,
            #[serde(default)]
            insert: HashMap<String, RawMapping>,
        }
        let raw = Raw::deserialize(d)?;
        let mut errors = Vec::new();
        let normal = compile_table("normal", raw.normal, &mut errors);
        let visual = compile_table("visual", raw.visual, &mut errors);
        let insert = compile_table("insert", raw.insert, &mut errors);
        // The tables iterate in HashMap order; sort so the startup notice
        // names the same entry on every launch.
        errors.sort();
        Ok(Self {
            normal,
            visual,
            insert,
            timeout: Duration::from_millis(raw.timeout),
            errors,
        })
    }
}

fn default_timeout_ms() -> u64 {
    DEFAULT_TIMEOUT_MS
}

fn compile_table(
    mode: &str,
    entries: HashMap<String, RawMapping>,
    errors: &mut Vec<String>,
) -> Table {
    let mut table = Table::new();
    for (lhs, raw) in entries {
        let (rhs, desc) = match raw {
            RawMapping::Keys(keys) => (keys, None),
            RawMapping::Described { keys, desc } => (keys, desc),
        };
        match compile(&lhs, &rhs) {
            Ok((ids, keys)) => {
                table.insert(ids, Mapping { keys, desc });
            }
            Err(e) => errors.push(format!("[keymaps.{mode}] {lhs}: {e}")),
        }
    }
    table
}

/// How a key reads in the which-key popup: plain characters as themselves
/// and `<space>` like the built-in rows, Vim notation for everything else.
fn key_label(&(code, mods): &KeyId) -> String {
    let name = match code {
        KeyCode::Char(' ') => "space".to_string(),
        KeyCode::Char(c) if mods.is_empty() => return c.to_string(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => "CR".into(),
        KeyCode::Esc => "Esc".into(),
        KeyCode::Tab => "Tab".into(),
        KeyCode::BackTab => "S-Tab".into(),
        KeyCode::Backspace => "BS".into(),
        KeyCode::Delete => "Del".into(),
        KeyCode::Up => "Up".into(),
        KeyCode::Down => "Down".into(),
        KeyCode::Left => "Left".into(),
        KeyCode::Right => "Right".into(),
        KeyCode::Home => "Home".into(),
        KeyCode::End => "End".into(),
        KeyCode::PageUp => "PageUp".into(),
        KeyCode::PageDown => "PageDown".into(),
        KeyCode::Insert => "Ins".into(),
        KeyCode::F(n) => format!("F{n}"),
        other => format!("{other:?}"),
    };
    let mut label = String::from("<");
    if mods.contains(KeyModifiers::CONTROL) {
        label.push_str("C-");
    }
    if mods.contains(KeyModifiers::ALT) {
        label.push_str("A-");
    }
    if mods.contains(KeyModifiers::SUPER) {
        label.push_str("D-");
    }
    label.push_str(&name);
    label.push('>');
    label
}

fn compile(lhs: &str, rhs: &str) -> Result<(Vec<KeyId>, Vec<KeyEvent>), String> {
    let from: Vec<KeyId> = parse_keys(lhs)?.iter().map(key_id).collect();
    if from.is_empty() {
        return Err("no key to map".into());
    }
    if rhs.eq_ignore_ascii_case("<nop>") {
        return Ok((from, Vec::new()));
    }
    let to = parse_keys(rhs)?;
    if to.is_empty() {
        return Err(r#"empty — map it to "<Nop>" to switch the key off"#.into());
    }
    Ok((from, to))
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

    fn keys(s: &str) -> Vec<KeyEvent> {
        s.chars().map(plain).collect()
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
            "<leader>w" = ":w<CR>"
            X = "<Spce>"
            Z = ""
            "" = "x"

            [visual]
            H = "^"
            "#,
        )
        .expect("a bad entry must not fail the whole table");
        assert_eq!(
            keymaps.exact(MapMode::Normal, &keys("H")),
            Some(&[plain('^')][..])
        );
        assert_eq!(
            keymaps.exact(MapMode::Normal, &keys("J")).map(<[_]>::len),
            Some(3)
        );
        assert!(keymaps.exact(MapMode::Normal, &keys("jk")).is_some());
        assert!(keymaps.exact(MapMode::Normal, &keys(" w")).is_some());
        assert!(keymaps.exact(MapMode::Visual, &keys("H")).is_some());
        assert!(keymaps.exact(MapMode::Visual, &keys("J")).is_none());
        assert_eq!(keymaps.errors.len(), 3, "{:?}", keymaps.errors);
        for lhs in ["X", "Z", ""] {
            let prefix = format!("[keymaps.normal] {lhs}:");
            assert!(
                keymaps.errors.iter().any(|e| e.starts_with(&prefix)),
                "{lhs:?} not reported in {:?}",
                keymaps.errors
            );
        }
    }

    #[test]
    fn lookup_holds_prefixes_and_resolves_full_matches() {
        let keymaps: Keymaps =
            toml::from_str("[normal]\ngh = \"^\"\nJ = \"j\"\nJk = \"gg\"").unwrap();
        let n = MapMode::Normal;
        assert!(matches!(
            keymaps.lookup(n, &keys("g")),
            KeymapMatch::Pending
        ));
        assert!(matches!(
            keymaps.lookup(n, &keys("gh")),
            KeymapMatch::Full(_)
        ));
        assert!(matches!(keymaps.lookup(n, &keys("gx")), KeymapMatch::None));
        assert!(matches!(keymaps.lookup(n, &keys("x")), KeymapMatch::None));
        // `J` is a mapping of its own, but `Jk` could still follow.
        assert!(matches!(
            keymaps.lookup(n, &keys("J")),
            KeymapMatch::Pending
        ));
        assert!(keymaps.exact(n, &keys("J")).is_some());
    }

    #[test]
    fn timeout_defaults_to_vims_and_can_be_set() {
        assert_eq!(Keymaps::default().timeout, Duration::from_millis(1000));
        let keymaps: Keymaps = toml::from_str("[normal]\nH = \"^\"").unwrap();
        assert_eq!(keymaps.timeout, Duration::from_millis(1000));
        let keymaps: Keymaps = toml::from_str("timeout = 300").unwrap();
        assert_eq!(keymaps.timeout, Duration::from_millis(300));
    }

    #[test]
    fn nop_switches_a_key_off() {
        let keymaps: Keymaps = toml::from_str("[normal]\nH = \"<Nop>\"").unwrap();
        assert_eq!(keymaps.exact(MapMode::Normal, &keys("H")), Some(&[][..]));
        assert!(keymaps.errors.is_empty());
    }

    #[test]
    fn lookup_ignores_how_the_terminal_reports_shift() {
        let keymaps: Keymaps = toml::from_str("[normal]\nH = \"^\"\n\"<C-r>\" = \"u\"").unwrap();
        let shifted_h = KeyEvent::new(KeyCode::Char('H'), KeyModifiers::SHIFT);
        assert!(keymaps.exact(MapMode::Normal, &[shifted_h]).is_some());
        let ctrl_shift_r = KeyEvent::new(
            KeyCode::Char('R'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );
        assert!(keymaps.exact(MapMode::Normal, &[ctrl_shift_r]).is_some());
    }

    #[test]
    fn a_mapping_can_carry_a_description() {
        let keymaps: Keymaps = toml::from_str(
            r#"
            [normal]
            "<leader>w" = { keys = ":w<CR>", desc = "Save" }
            "<leader>q" = { keys = ":q<CR>" }
            "#,
        )
        .unwrap();
        assert!(keymaps.errors.is_empty(), "{:?}", keymaps.errors);
        assert_eq!(
            keymaps.exact(MapMode::Normal, &keys(" w")).map(<[_]>::len),
            Some(3)
        );
        let mut rows = vec![("w".to_string(), "Built-in".to_string())];
        keymaps.merge_whichkey(MapMode::Normal, &keys(" "), &mut rows);
        assert_eq!(
            rows,
            vec![
                ("w".to_string(), "Save".to_string()),
                ("q".to_string(), ":q<CR>".to_string()),
            ]
        );
    }

    #[test]
    fn whichkey_rows_only_cover_the_next_key() {
        let keymaps: Keymaps = toml::from_str(
            "[normal]\n\"<leader>xy\" = \"gg\"\n\"<leader>z\" = \"<Nop>\"\n\"<C-x>\" = \"u\"",
        )
        .unwrap();
        let mut rows = Vec::new();
        keymaps.merge_whichkey(MapMode::Normal, &keys(" "), &mut rows);
        assert_eq!(rows, vec![("z".to_string(), "<Nop>".to_string())]);
    }

    #[test]
    fn key_labels_match_the_built_in_rows() {
        assert_eq!(key_label(&key_id(&plain(' '))), "<space>");
        assert_eq!(key_label(&key_id(&plain('G'))), "G");
        assert_eq!(
            key_label(&(KeyCode::Char('x'), KeyModifiers::CONTROL)),
            "<C-x>"
        );
        assert_eq!(key_label(&(KeyCode::Enter, KeyModifiers::NONE)), "<CR>");
    }
}
