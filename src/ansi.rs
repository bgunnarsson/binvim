//! Minimal ANSI / SGR escape parser. Used by:
//!
//!   - `src/terminal.rs` — color lookup tables for the vte-driven
//!     `:terminal` pane (it does its own full CSI handling but reuses
//!     `ansi_basic_colour` / `ansi_bright_colour` / `ansi_256` for the
//!     palette).
//!   - `src/render.rs` — `parse_sgr_line` converts DAP `output` lines
//!     into styled segments so the debug Console tab honours the
//!     ANSI colour escapes most loggers and test runners emit.
//!
//! Deliberately small. We only handle SGR (`\x1b[…m`) and silently
//! drop other CSI sequences (cursor moves, ED/EL, etc.) plus OSC
//! (`\x1b]…BEL` / `\x1b]…\x1b\\`). Anything else passes through as
//! literal characters. The full xterm grammar lives in the vte
//! crate; this module covers the subset that shows up in
//! line-oriented logger output.
//!
//! `parse_sgr_line` is stateless across lines on purpose. The DAP
//! Console glues `output` events that may not align with newlines,
//! but in practice loggers either flush their colour on each line
//! or re-emit the SGR with each record — so per-line parsing reads
//! correctly in the common case and avoids carrying parser state
//! across logically unrelated output events.

use crossterm::style::Color;

/// One run of text sharing the same SGR styling. `fg = None` means
/// "default foreground" — the caller paints it with the pane's text
/// colour. Bold / italic map to the SGR `\x1b[1m` / `\x1b[3m` flags
/// and their resets (`\x1b[22m` / `\x1b[23m`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SgrSegment {
    pub text: String,
    pub fg: Option<Color>,
    pub bold: bool,
    pub italic: bool,
}

/// True if `s` contains at least one ESC (`0x1b`). Cheap pre-check
/// so `build_console_rows` can short-circuit straight to the
/// tokeniser for the common no-escape case.
pub fn has_escapes(s: &str) -> bool {
    s.as_bytes().contains(&0x1b)
}

/// Convert `line` into a list of styled segments. Lines without any
/// ANSI escapes return a single segment with default styling. Lines
/// with escapes are split on every SGR boundary; non-SGR escapes
/// (cursor moves, clears, OSC) are dropped silently.
pub fn parse_sgr_line(line: &str) -> Vec<SgrSegment> {
    let mut out: Vec<SgrSegment> = Vec::new();
    let mut style = SgrStyle::default();
    let mut buf = String::new();
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        // Walk by char boundaries so we don't slice mid-codepoint.
        // ANSI escapes are all ASCII so the byte-level scan is safe;
        // for plain text we step by char.
        if bytes[i] == 0x1b {
            // ESC. Inspect the next byte to decide which family.
            match bytes.get(i + 1) {
                Some(&b'[') => {
                    let (end, final_byte) = scan_csi(bytes, i + 2);
                    if final_byte == Some(b'm') {
                        // SGR — flush current segment, then update style.
                        flush_segment(&mut out, &mut buf, &style);
                        let params = &line[i + 2..end];
                        apply_sgr_params(&mut style, params);
                    }
                    // Skip the CSI sequence regardless of final byte.
                    i = end + 1;
                    continue;
                }
                Some(&b']') => {
                    // OSC — `\x1b]…BEL` or `\x1b]…\x1b\\`. Drop.
                    i = scan_osc(bytes, i + 2);
                    continue;
                }
                Some(_) => {
                    // Other ESC sequences (`\x1b(B`, `\x1bM`, etc.).
                    // Drop ESC + one byte and continue.
                    i += 2;
                    continue;
                }
                None => {
                    // Stray ESC at end-of-line — drop.
                    i += 1;
                    continue;
                }
            }
        }
        // Plain text — walk one UTF-8 char at a time so multibyte
        // glyphs (logger output is full of UTF-8 paths / emoji)
        // make it through intact.
        let ch = line[i..].chars().next().unwrap_or('\0');
        buf.push(ch);
        i += ch.len_utf8();
    }
    flush_segment(&mut out, &mut buf, &style);
    if out.is_empty() {
        // Pure-escape line (or empty input) — preserve a single
        // empty segment so the caller still emits a row.
        out.push(SgrSegment {
            text: String::new(),
            fg: style.fg,
            bold: style.bold,
            italic: style.italic,
        });
    }
    out
}

/// Internal pen state during a parse. Mirrors the SGR subset we
/// actually round-trip into `SgrSegment` (no underline / reverse —
/// the Console paint path can't render either right now).
#[derive(Debug, Clone, Copy, Default)]
struct SgrStyle {
    fg: Option<Color>,
    bold: bool,
    italic: bool,
}

fn flush_segment(out: &mut Vec<SgrSegment>, buf: &mut String, style: &SgrStyle) {
    if buf.is_empty() {
        return;
    }
    out.push(SgrSegment {
        text: std::mem::take(buf),
        fg: style.fg,
        bold: style.bold,
        italic: style.italic,
    });
}

/// Walk a CSI parameter / intermediate run from `start` until the
/// final byte (0x40..=0x7E). Returns `(final_byte_idx, Some(byte))`.
/// If the sequence is truncated, returns `(bytes.len(), None)`.
fn scan_csi(bytes: &[u8], start: usize) -> (usize, Option<u8>) {
    let mut j = start;
    while j < bytes.len() {
        let b = bytes[j];
        if (0x40..=0x7e).contains(&b) {
            return (j, Some(b));
        }
        j += 1;
    }
    (bytes.len(), None)
}

/// Walk an OSC body from `start` until ST (`\x1b\\`) or BEL (`0x07`).
/// Returns the index just past the terminator.
fn scan_osc(bytes: &[u8], start: usize) -> usize {
    let mut j = start;
    while j < bytes.len() {
        match bytes[j] {
            0x07 => return j + 1,
            0x1b if bytes.get(j + 1) == Some(&b'\\') => return j + 2,
            _ => j += 1,
        }
    }
    bytes.len()
}

fn apply_sgr_params(style: &mut SgrStyle, params: &str) {
    // Empty params (`\x1b[m`) is equivalent to `\x1b[0m`.
    if params.is_empty() {
        *style = SgrStyle::default();
        return;
    }
    // SGR uses `;` as the primary separator; the colon-sub-param form
    // (`38:2:R:G:B`) shows up too. We walk by primary `;` segments
    // and look inside each one for either a bare integer or a colon
    // chain. Anything we can't parse is silently dropped.
    let mut segs = params.split(';').peekable();
    while let Some(seg) = segs.next() {
        // Colon form keeps the whole 38/48 chain in one `seg`.
        if let Some((head, _rest)) = seg.split_once(':') {
            let n = head.parse::<u16>().unwrap_or(0);
            match n {
                38 => style.fg = parse_colon_extended(seg).or(style.fg),
                48 => { /* bg — not modelled in DapTabPart */ }
                _ => apply_simple(style, n),
            }
            continue;
        }
        let n = seg.parse::<u16>().unwrap_or(0);
        match n {
            38 => style.fg = parse_semicolon_extended(&mut segs).or(style.fg),
            48 => {
                // Consume bg's parameters off the iterator so we don't
                // misread them as the next attribute.
                let _ = parse_semicolon_extended(&mut segs);
            }
            _ => apply_simple(style, n),
        }
    }
}

/// Apply a one-word SGR parameter (0/1/22/3/23/30-37/39/90-97). Bg
/// ranges (40-47/100-107) silently no-op because `DapTabPart` has no
/// bg slot.
fn apply_simple(style: &mut SgrStyle, n: u16) {
    match n {
        0 => *style = SgrStyle::default(),
        1 => style.bold = true,
        3 => style.italic = true,
        22 => style.bold = false,
        23 => style.italic = false,
        39 => style.fg = None,
        30..=37 => style.fg = Some(ansi_basic_colour(n - 30)),
        90..=97 => style.fg = Some(ansi_bright_colour(n - 90)),
        _ => {}
    }
}

fn parse_colon_extended(seg: &str) -> Option<Color> {
    // `38:5:N` or `38:2[:cs]:R:G:B`. We already know the head is 38.
    let mut parts = seg.split(':');
    let _head = parts.next();
    let kind = parts.next()?.parse::<u16>().ok()?;
    match kind {
        5 => {
            let idx = parts.next()?.parse::<u16>().ok()? as u8;
            Some(ansi_256(idx))
        }
        2 => {
            // The full CSI shape is `38:2::R:G:B` (with a colour-space
            // sentinel slot) — but `38:2:R:G:B` is also accepted in
            // the wild. Pull the remaining parts; if there are four
            // left, the first is the sentinel and we skip it; if
            // there are three, they're already R/G/B.
            let rest: Vec<&str> = parts.collect();
            let (r_idx, g_idx, b_idx) = if rest.len() >= 4 {
                (1, 2, 3)
            } else if rest.len() == 3 {
                (0, 1, 2)
            } else {
                return None;
            };
            let r = rest.get(r_idx)?.parse::<u16>().ok()? as u8;
            let g = rest.get(g_idx)?.parse::<u16>().ok()? as u8;
            let b = rest.get(b_idx)?.parse::<u16>().ok()? as u8;
            Some(Color::Rgb { r, g, b })
        }
        _ => None,
    }
}

fn parse_semicolon_extended<'a>(
    segs: &mut std::iter::Peekable<std::str::Split<'a, char>>,
) -> Option<Color> {
    let kind = segs.next()?.parse::<u16>().ok()?;
    match kind {
        5 => {
            let idx = segs.next()?.parse::<u16>().ok()? as u8;
            Some(ansi_256(idx))
        }
        2 => {
            let r = segs.next()?.parse::<u16>().ok()? as u8;
            let g = segs.next()?.parse::<u16>().ok()? as u8;
            let b = segs.next()?.parse::<u16>().ok()? as u8;
            Some(Color::Rgb { r, g, b })
        }
        _ => None,
    }
}

pub fn ansi_basic_colour(n: u16) -> Color {
    match n {
        0 => Color::Black,
        1 => Color::DarkRed,
        2 => Color::DarkGreen,
        3 => Color::DarkYellow,
        4 => Color::DarkBlue,
        5 => Color::DarkMagenta,
        6 => Color::DarkCyan,
        7 => Color::Grey,
        _ => Color::Reset,
    }
}

pub fn ansi_bright_colour(n: u16) -> Color {
    match n {
        0 => Color::DarkGrey,
        1 => Color::Red,
        2 => Color::Green,
        3 => Color::Yellow,
        4 => Color::Blue,
        5 => Color::Magenta,
        6 => Color::Cyan,
        7 => Color::White,
        _ => Color::Reset,
    }
}

pub fn ansi_256(idx: u8) -> Color {
    // Standard xterm 256-colour cube: 0-15 are the basic + bright
    // palette, 16-231 are a 6×6×6 RGB cube, 232-255 is a 24-step
    // grayscale ramp. Translate to truecolor so the renderer can
    // paint without consulting a separate lookup.
    if idx < 8 {
        return ansi_basic_colour(idx as u16);
    }
    if idx < 16 {
        return ansi_bright_colour((idx - 8) as u16);
    }
    if idx < 232 {
        let n = idx - 16;
        let r = n / 36;
        let g = (n % 36) / 6;
        let b = n % 6;
        let scale = |v: u8| if v == 0 { 0 } else { 55 + 40 * v };
        return Color::Rgb {
            r: scale(r),
            g: scale(g),
            b: scale(b),
        };
    }
    let v = 8 + (idx - 232) * 10;
    Color::Rgb { r: v, g: v, b: v }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_returns_one_segment() {
        let segs = parse_sgr_line("hello world");
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].text, "hello world");
        assert_eq!(segs[0].fg, None);
        assert!(!segs[0].bold);
        assert!(!segs[0].italic);
    }

    #[test]
    fn has_escapes_detects_esc_byte() {
        assert!(!has_escapes("plain"));
        assert!(has_escapes("\x1b[31mred"));
    }

    #[test]
    fn basic_red_then_reset() {
        let segs = parse_sgr_line("\x1b[31mERROR\x1b[0m: oops");
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].text, "ERROR");
        assert_eq!(segs[0].fg, Some(Color::DarkRed));
        assert_eq!(segs[1].text, ": oops");
        assert_eq!(segs[1].fg, None);
    }

    #[test]
    fn bright_yellow_with_bold() {
        let segs = parse_sgr_line("\x1b[1;93mWARN\x1b[0m tail");
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].text, "WARN");
        assert_eq!(segs[0].fg, Some(Color::Yellow));
        assert!(segs[0].bold);
        assert!(!segs[1].bold);
    }

    #[test]
    fn semicolon_256_color() {
        let segs = parse_sgr_line("\x1b[38;5;202mORANGE\x1b[0m");
        assert_eq!(segs.len(), 1);
        assert!(matches!(segs[0].fg, Some(Color::Rgb { .. })));
    }

    #[test]
    fn semicolon_truecolor() {
        let segs = parse_sgr_line("\x1b[38;2;255;100;50mRGB\x1b[0m");
        assert_eq!(segs.len(), 1);
        assert_eq!(
            segs[0].fg,
            Some(Color::Rgb {
                r: 255,
                g: 100,
                b: 50
            })
        );
    }

    #[test]
    fn colon_truecolor_with_trailing_attr() {
        // opencode-style: `\x1b[38:2:R:G:B;24m` — 24 is "no underline".
        // We don't model underline, but the parser must not get
        // confused by the trailing token.
        let segs = parse_sgr_line("\x1b[38:2:10:20:30;1mX\x1b[0m");
        assert_eq!(segs.len(), 1);
        assert_eq!(
            segs[0].fg,
            Some(Color::Rgb {
                r: 10,
                g: 20,
                b: 30
            })
        );
        assert!(segs[0].bold);
    }

    #[test]
    fn italic_then_reset_italic() {
        let segs = parse_sgr_line("\x1b[3mit\x1b[23m off");
        assert_eq!(segs.len(), 2);
        assert!(segs[0].italic);
        assert!(!segs[1].italic);
    }

    #[test]
    fn empty_params_resets() {
        let segs = parse_sgr_line("\x1b[31mred\x1b[mclear");
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].fg, Some(Color::DarkRed));
        assert_eq!(segs[1].fg, None);
    }

    #[test]
    fn cursor_moves_are_dropped() {
        // `\x1b[2J` (clear screen) and `\x1b[H` (cursor home) leak in
        // from poorly-flushed adapter output; they must not show up
        // as literal text in the Console.
        let segs = parse_sgr_line("before\x1b[2J\x1b[Hafter");
        let joined: String = segs.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, "beforeafter");
    }

    #[test]
    fn osc_hyperlink_is_dropped() {
        // OSC 8 hyperlinks: `\x1b]8;;URL\x1b\\TEXT\x1b]8;;\x1b\\`.
        let segs = parse_sgr_line("\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\");
        let joined: String = segs.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, "link");
    }

    #[test]
    fn osc_with_bel_terminator_is_dropped() {
        let segs = parse_sgr_line("a\x1b]0;title\x07b");
        let joined: String = segs.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(joined, "ab");
    }

    #[test]
    fn unknown_sgr_codes_are_skipped() {
        // `\x1b[99m` is undefined — should not crash, should not
        // affect colour state, should produce no visible output.
        let segs = parse_sgr_line("\x1b[99mtext");
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].text, "text");
        assert_eq!(segs[0].fg, None);
    }

    #[test]
    fn utf8_passes_through_intact() {
        let segs = parse_sgr_line("\x1b[32m✓ ok 한글\x1b[0m");
        assert_eq!(segs[0].text, "✓ ok 한글");
        assert_eq!(segs[0].fg, Some(Color::DarkGreen));
    }

    #[test]
    fn standalone_default_fg_resets() {
        let segs = parse_sgr_line("\x1b[31mred\x1b[39mdefault");
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].fg, Some(Color::DarkRed));
        assert_eq!(segs[1].fg, None);
    }

    #[test]
    fn bg_parameter_is_consumed_not_treated_as_fg() {
        // `\x1b[48;5;202m` is a 256-colour background. It must not
        // accidentally set fg, and the trailing param walk must
        // not get out of sync.
        let segs = parse_sgr_line("\x1b[48;5;202;31mtext\x1b[0m");
        assert_eq!(segs[0].fg, Some(Color::DarkRed));
    }
}
