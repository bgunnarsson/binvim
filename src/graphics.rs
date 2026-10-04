//! Images drawn in the terminal, over the kitty graphics protocol's Unicode
//! placeholders — kitty's own, and Ghostty's, which took it from kitty.
//!
//! A placeholder image is text: the image is sent once under an id, and each
//! cell it covers is a U+10EEEE carrying its row and column as combining
//! marks, coloured with the id. binvim repaints the whole frame from text
//! each time, so that's the shape that fits — scrolling, a cell half off
//! screen and a popup drawn over it all fall out of drawing those cells or
//! not, where a placed image would have to be moved and deleted in step.
//! A terminal without placeholders gets the text the caller draws instead.

use std::collections::{HashMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::Write;
use std::rc::Rc;

use crossterm::style::Color;

/// Kitty's `rowcolumn-diacritics.txt`: the mark for row or column `n` is
/// the `n`th entry, which caps an image at this many rows and columns.
const DIACRITICS: &[char] = &[
    '\u{0305}',
    '\u{030D}',
    '\u{030E}',
    '\u{0310}',
    '\u{0312}',
    '\u{033D}',
    '\u{033E}',
    '\u{033F}',
    '\u{0346}',
    '\u{034A}',
    '\u{034B}',
    '\u{034C}',
    '\u{0350}',
    '\u{0351}',
    '\u{0352}',
    '\u{0357}',
    '\u{035B}',
    '\u{0363}',
    '\u{0364}',
    '\u{0365}',
    '\u{0366}',
    '\u{0367}',
    '\u{0368}',
    '\u{0369}',
    '\u{036A}',
    '\u{036B}',
    '\u{036C}',
    '\u{036D}',
    '\u{036E}',
    '\u{036F}',
    '\u{0483}',
    '\u{0484}',
    '\u{0485}',
    '\u{0486}',
    '\u{0487}',
    '\u{0592}',
    '\u{0593}',
    '\u{0594}',
    '\u{0595}',
    '\u{0597}',
    '\u{0598}',
    '\u{0599}',
    '\u{059C}',
    '\u{059D}',
    '\u{059E}',
    '\u{059F}',
    '\u{05A0}',
    '\u{05A1}',
    '\u{05A8}',
    '\u{05A9}',
    '\u{05AB}',
    '\u{05AC}',
    '\u{05AF}',
    '\u{05C4}',
    '\u{0610}',
    '\u{0611}',
    '\u{0612}',
    '\u{0613}',
    '\u{0614}',
    '\u{0615}',
    '\u{0616}',
    '\u{0617}',
    '\u{0657}',
    '\u{0658}',
    '\u{0659}',
    '\u{065A}',
    '\u{065B}',
    '\u{065D}',
    '\u{065E}',
    '\u{06D6}',
    '\u{06D7}',
    '\u{06D8}',
    '\u{06D9}',
    '\u{06DA}',
    '\u{06DB}',
    '\u{06DC}',
    '\u{06DF}',
    '\u{06E0}',
    '\u{06E1}',
    '\u{06E2}',
    '\u{06E4}',
    '\u{06E7}',
    '\u{06E8}',
    '\u{06EB}',
    '\u{06EC}',
    '\u{0730}',
    '\u{0732}',
    '\u{0733}',
    '\u{0735}',
    '\u{0736}',
    '\u{073A}',
    '\u{073D}',
    '\u{073F}',
    '\u{0740}',
    '\u{0741}',
    '\u{0743}',
    '\u{0745}',
    '\u{0747}',
    '\u{0749}',
    '\u{074A}',
    '\u{07EB}',
    '\u{07EC}',
    '\u{07ED}',
    '\u{07EE}',
    '\u{07EF}',
    '\u{07F0}',
    '\u{07F1}',
    '\u{07F3}',
    '\u{0816}',
    '\u{0817}',
    '\u{0818}',
    '\u{0819}',
    '\u{081B}',
    '\u{081C}',
    '\u{081D}',
    '\u{081E}',
    '\u{081F}',
    '\u{0820}',
    '\u{0821}',
    '\u{0822}',
    '\u{0823}',
    '\u{0825}',
    '\u{0826}',
    '\u{0827}',
    '\u{0829}',
    '\u{082A}',
    '\u{082B}',
    '\u{082C}',
    '\u{082D}',
    '\u{0951}',
    '\u{0953}',
    '\u{0954}',
    '\u{0F82}',
    '\u{0F83}',
    '\u{0F86}',
    '\u{0F87}',
    '\u{135D}',
    '\u{135E}',
    '\u{135F}',
    '\u{17DD}',
    '\u{193A}',
    '\u{1A17}',
    '\u{1A75}',
    '\u{1A76}',
    '\u{1A77}',
    '\u{1A78}',
    '\u{1A79}',
    '\u{1A7A}',
    '\u{1A7B}',
    '\u{1A7C}',
    '\u{1B6B}',
    '\u{1B6D}',
    '\u{1B6E}',
    '\u{1B6F}',
    '\u{1B70}',
    '\u{1B71}',
    '\u{1B72}',
    '\u{1B73}',
    '\u{1CD0}',
    '\u{1CD1}',
    '\u{1CD2}',
    '\u{1CDA}',
    '\u{1CDB}',
    '\u{1CE0}',
    '\u{1DC0}',
    '\u{1DC1}',
    '\u{1DC3}',
    '\u{1DC4}',
    '\u{1DC5}',
    '\u{1DC6}',
    '\u{1DC7}',
    '\u{1DC8}',
    '\u{1DC9}',
    '\u{1DCB}',
    '\u{1DCC}',
    '\u{1DD1}',
    '\u{1DD2}',
    '\u{1DD3}',
    '\u{1DD4}',
    '\u{1DD5}',
    '\u{1DD6}',
    '\u{1DD7}',
    '\u{1DD8}',
    '\u{1DD9}',
    '\u{1DDA}',
    '\u{1DDB}',
    '\u{1DDC}',
    '\u{1DDD}',
    '\u{1DDE}',
    '\u{1DDF}',
    '\u{1DE0}',
    '\u{1DE1}',
    '\u{1DE2}',
    '\u{1DE3}',
    '\u{1DE4}',
    '\u{1DE5}',
    '\u{1DE6}',
    '\u{1DFE}',
    '\u{20D0}',
    '\u{20D1}',
    '\u{20D4}',
    '\u{20D5}',
    '\u{20D6}',
    '\u{20D7}',
    '\u{20DB}',
    '\u{20DC}',
    '\u{20E1}',
    '\u{20E7}',
    '\u{20E9}',
    '\u{20F0}',
    '\u{2CEF}',
    '\u{2CF0}',
    '\u{2CF1}',
    '\u{2DE0}',
    '\u{2DE1}',
    '\u{2DE2}',
    '\u{2DE3}',
    '\u{2DE4}',
    '\u{2DE5}',
    '\u{2DE6}',
    '\u{2DE7}',
    '\u{2DE8}',
    '\u{2DE9}',
    '\u{2DEA}',
    '\u{2DEB}',
    '\u{2DEC}',
    '\u{2DED}',
    '\u{2DEE}',
    '\u{2DEF}',
    '\u{2DF0}',
    '\u{2DF1}',
    '\u{2DF2}',
    '\u{2DF3}',
    '\u{2DF4}',
    '\u{2DF5}',
    '\u{2DF6}',
    '\u{2DF7}',
    '\u{2DF8}',
    '\u{2DF9}',
    '\u{2DFA}',
    '\u{2DFB}',
    '\u{2DFC}',
    '\u{2DFD}',
    '\u{2DFE}',
    '\u{2DFF}',
    '\u{A66F}',
    '\u{A67C}',
    '\u{A67D}',
    '\u{A6F0}',
    '\u{A6F1}',
    '\u{A8E0}',
    '\u{A8E1}',
    '\u{A8E2}',
    '\u{A8E3}',
    '\u{A8E4}',
    '\u{A8E5}',
    '\u{A8E6}',
    '\u{A8E7}',
    '\u{A8E8}',
    '\u{A8E9}',
    '\u{A8EA}',
    '\u{A8EB}',
    '\u{A8EC}',
    '\u{A8ED}',
    '\u{A8EE}',
    '\u{A8EF}',
    '\u{A8F0}',
    '\u{A8F1}',
    '\u{AAB0}',
    '\u{AAB2}',
    '\u{AAB3}',
    '\u{AAB7}',
    '\u{AAB8}',
    '\u{AABE}',
    '\u{AABF}',
    '\u{AAC1}',
    '\u{FE20}',
    '\u{FE21}',
    '\u{FE22}',
    '\u{FE23}',
    '\u{FE24}',
    '\u{FE25}',
    '\u{FE26}',
    '\u{10A0F}',
    '\u{10A38}',
    '\u{1D185}',
    '\u{1D186}',
    '\u{1D187}',
    '\u{1D188}',
    '\u{1D189}',
    '\u{1D1AA}',
    '\u{1D1AB}',
    '\u{1D1AC}',
    '\u{1D1AD}',
    '\u{1D242}',
    '\u{1D243}',
    '\u{1D244}',
];

const PLACEHOLDER: char = '\u{10EEEE}';
/// Taller than this and an image is scaled down to fit, so one plot doesn't
/// fill several screens.
const MAX_ROWS: usize = 30;
/// Larger images are scaled down before they're sent: a phone photo is
/// megabytes of base64 to show in a few dozen cells.
const MAX_PX: u32 = 2048;
/// Files past this aren't read as images at all.
pub const MAX_FILE: u64 = 32 << 20;

/// True when the terminal draws placeholder images. Read from the
/// environment, since asking the terminal means reading its answer off the
/// same input the event loop reads keys from. tmux passes placeholders
/// through only when configured to, so it gets the text.
pub fn terminal_supports() -> bool {
    let var = |k: &str| std::env::var(k).unwrap_or_default();
    if std::env::var_os("TMUX").is_some() {
        return false;
    }
    var("TERM_PROGRAM") == "ghostty"
        || var("TERM") == "xterm-ghostty"
        || var("TERM") == "xterm-kitty"
        || std::env::var_os("KITTY_WINDOW_ID").is_some()
}

struct Decoded {
    png: Rc<[u8]>,
    w: u32,
    h: u32,
}

/// Every image the page has asked for, decoded once, and which of them the
/// terminal holds. An image gets an id per size it's shown at; a new size
/// (a resized pane) sends it again and deletes the old one.
pub struct ImageStore {
    decoded: HashMap<String, Option<Decoded>>,
    ids: HashMap<String, (u32, usize, usize)>,
    by_id: HashMap<u32, String>,
    sent: HashSet<u32>,
    stale: Vec<u32>,
    next_id: u32,
}

impl Default for ImageStore {
    fn default() -> Self {
        Self {
            decoded: HashMap::new(),
            ids: HashMap::new(),
            by_id: HashMap::new(),
            sent: HashSet::new(),
            stale: Vec::new(),
            next_id: 1,
        }
    }
}

impl ImageStore {
    /// The size in pixels of the image `key` names, read by `load` the
    /// first time it's asked for. `None` when it doesn't decode.
    pub fn size(
        &mut self,
        key: &str,
        load: impl FnOnce() -> Option<Vec<u8>>,
    ) -> Option<(u32, u32)> {
        if !self.decoded.contains_key(key) {
            let decoded = load().and_then(|bytes| decode(&bytes));
            self.decoded.insert(key.to_string(), decoded);
        }
        self.decoded.get(key)?.as_ref().map(|d| (d.w, d.h))
    }

    /// The id the image `key` is shown under at `cols` × `rows`.
    pub fn id(&mut self, key: &str, cols: usize, rows: usize) -> u32 {
        if let Some(&(id, c, r)) = self.ids.get(key) {
            if (c, r) == (cols, rows) {
                return id;
            }
            self.by_id.remove(&id);
            if self.sent.remove(&id) {
                self.stale.push(id);
            }
        }
        let id = self.next_id;
        // The id travels as a 24-bit colour.
        self.next_id = if id >= 0xFF_FFFF { 1 } else { id + 1 };
        self.ids.insert(key.to_string(), (id, cols, rows));
        self.by_id.insert(id, key.to_string());
        id
    }

    /// Sends image `id` to the terminal unless it already has it, and drops
    /// the ones a resize replaced.
    pub fn send(&mut self, out: &mut impl Write, id: u32) -> std::io::Result<()> {
        for old in self.stale.drain(..) {
            write!(out, "\x1b_Ga=d,d=I,i={old},q=2\x1b\\")?;
        }
        if self.sent.contains(&id) {
            return Ok(());
        }
        let Some(key) = self.by_id.get(&id) else { return Ok(()) };
        let Some(&(_, cols, rows)) = self.ids.get(key) else { return Ok(()) };
        let Some(Some(d)) = self.decoded.get(key) else { return Ok(()) };
        write_transmit(out, id, cols, rows, &d.png)?;
        self.sent.insert(id);
        Ok(())
    }

    /// The terminal dropped its images: it left the alternate screen for
    /// lazygit, yazi or an install and came back to a fresh one.
    pub fn forget_sent(&mut self) {
        self.sent.clear();
        self.stale.clear();
    }
}

/// Cells for a `w` × `h` image no wider than `max_cols`: its own size in
/// the terminal's cells, scaled down to fit.
pub fn fit(w: u32, h: u32, max_cols: usize) -> (usize, usize) {
    let (cw, ch) = cell_px();
    let (w, h) = (w.max(1) as f64, h.max(1) as f64);
    let max_cols = max_cols.clamp(1, DIACRITICS.len());
    let mut cols = ((w / cw).ceil() as usize).clamp(1, max_cols);
    let mut rows = ((cols as f64 * cw * h / w / ch).round() as usize).max(1);
    if rows > MAX_ROWS {
        rows = MAX_ROWS;
        cols = ((rows as f64 * ch * w / h / cw).round() as usize).clamp(1, max_cols);
    }
    (cols, rows)
}

/// A cell's size in pixels, from the window size the terminal reports; a
/// common one when it reports none.
fn cell_px() -> (f64, f64) {
    match crossterm::terminal::window_size() {
        Ok(s) if s.width > 0 && s.height > 0 && s.columns > 0 && s.rows > 0 => (
            s.width as f64 / s.columns as f64,
            s.height as f64 / s.rows as f64,
        ),
        _ => (10.0, 20.0),
    }
}

/// The text that draws row `row` of an image `cols` wide; it has to be
/// printed in `id_color` of the image's id.
pub fn placeholder_row(row: usize, cols: usize) -> String {
    let r = DIACRITICS[row.min(DIACRITICS.len() - 1)];
    (0..cols.min(DIACRITICS.len()))
        .flat_map(|c| [PLACEHOLDER, r, DIACRITICS[c]])
        .collect()
}

pub fn id_color(id: u32) -> Color {
    Color::Rgb {
        r: (id >> 16) as u8,
        g: (id >> 8) as u8,
        b: id as u8,
    }
}

/// A cache key for image bytes that are already in memory, like an output's.
pub fn content_key<'a>(parts: impl IntoIterator<Item = &'a str>) -> String {
    let mut h = DefaultHasher::new();
    for p in parts {
        p.hash(&mut h);
    }
    format!("data:{:016x}", h.finish())
}

/// PNG as it is, when it's small enough; anything else `image` reads,
/// re-encoded as PNG, since that's the one format the protocol takes.
fn decode(bytes: &[u8]) -> Option<Decoded> {
    if let Some((w, h)) = png_size(bytes) {
        if w <= MAX_PX && h <= MAX_PX {
            return Some(Decoded {
                png: bytes.into(),
                w,
                h,
            });
        }
    }
    let mut img = image::load_from_memory(bytes).ok()?;
    if img.width() > MAX_PX || img.height() > MAX_PX {
        img = img.thumbnail(MAX_PX, MAX_PX);
    }
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .ok()?;
    Some(Decoded {
        png: png.into(),
        w: img.width(),
        h: img.height(),
    })
}

fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") || bytes.get(12..16)? != b"IHDR" {
        return None;
    }
    let be = |r: std::ops::Range<usize>| Some(u32::from_be_bytes(bytes.get(r)?.try_into().ok()?));
    Some((be(16..20)?, be(20..24)?))
}

/// `a=T` with `U=1`: store the image and give it a virtual placement
/// `cols` × `rows` cells, which placeholders then draw. `q=2` keeps the
/// terminal from answering into binvim's input.
fn write_transmit(
    out: &mut impl Write,
    id: u32,
    cols: usize,
    rows: usize,
    png: &[u8],
) -> std::io::Result<()> {
    let b64 = base64(png);
    let chunks: Vec<&[u8]> = b64.as_bytes().chunks(4096).collect();
    for (i, chunk) in chunks.iter().enumerate() {
        let more = u8::from(i + 1 < chunks.len());
        if i == 0 {
            write!(
                out,
                "\x1b_Ga=T,U=1,f=100,i={id},c={cols},r={rows},q=2,m={more};"
            )?;
        } else {
            write!(out, "\x1b_Gm={more};")?;
        }
        out.write_all(chunk)?;
        out.write_all(b"\x1b\\")?;
    }
    Ok(())
}

fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, &b)| n | (b as u32) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ABC[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_pads_like_the_standard_encoding() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn the_table_is_kittys() {
        assert_eq!(DIACRITICS.len(), 297);
        assert_eq!(DIACRITICS[0], '\u{0305}');
        assert_eq!(DIACRITICS[296], '\u{1D244}');
    }

    #[test]
    fn a_placeholder_row_names_its_row_and_each_column() {
        assert_eq!(
            placeholder_row(1, 2),
            "\u{10EEEE}\u{030D}\u{0305}\u{10EEEE}\u{030D}\u{030D}"
        );
    }

    #[test]
    fn a_png_is_kept_and_its_size_read_from_its_header() {
        let mut png = Vec::new();
        image::RgbaImage::new(3, 2)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let d = decode(&png).unwrap();
        assert_eq!((d.w, d.h), (3, 2));
        assert_eq!(&*d.png, png.as_slice());
        assert!(decode(b"not an image").is_none());
    }

    #[test]
    fn a_new_size_gets_a_new_id_and_the_old_one_is_deleted() {
        let mut store = ImageStore::default();
        let mut png = Vec::new();
        image::RgbaImage::new(4, 4)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        assert_eq!(store.size("k", || Some(png.clone())), Some((4, 4)));
        let a = store.id("k", 2, 1);
        assert_eq!(store.id("k", 2, 1), a);
        let mut out = Vec::new();
        store.send(&mut out, a).unwrap();
        assert!(
            String::from_utf8_lossy(&out)
                .starts_with(&format!("\x1b_Ga=T,U=1,f=100,i={a},c=2,r=1,q=2,m=0;"))
        );
        out.clear();
        store.send(&mut out, a).unwrap();
        assert!(out.is_empty(), "sent once");
        let b = store.id("k", 4, 2);
        assert_ne!(a, b);
        store.send(&mut out, b).unwrap();
        assert!(
            String::from_utf8_lossy(&out).starts_with(&format!("\x1b_Ga=d,d=I,i={a},q=2\x1b\\"))
        );
    }
}
