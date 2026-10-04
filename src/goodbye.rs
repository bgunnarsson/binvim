//! The logo and parting line printed to the shell after a clean quit, once the
//! alternate screen is gone so they stay in the scrollback.

use crate::config::Config;
use crossterm::{
    queue,
    style::{Print, ResetColor, SetForegroundColor},
};
use std::io::{self, Write};
use std::time::{SystemTime, UNIX_EPOCH};

const MESSAGES: &[&str] = &[
    "You found the exit. Most vim users are still looking.",
    "Buffers closed, cursors parked. Go touch grass.",
    "Exited cleanly. Stack Overflow will not be needed today.",
    "Your undo history is safe with me. Your weekend plans are on you.",
    "Off you go. The cursor will keep blinking without you.",
    "May your diffs be small and your builds be green.",
    "You quit on the first try. Tell the others it can be done.",
    "No buffers were harmed in the making of this session.",
    "Logging off. Somewhere a semicolon is still missing.",
    "See you later, operator-pending.",
];

fn message(seed: u64) -> &'static str {
    MESSAGES[(seed % MESSAGES.len() as u64) as usize]
}

pub fn print(config: &Config) {
    if !config.start_page.goodbye {
        return;
    }
    let configured: Vec<&str> = config.start_page.lines.iter().map(|s| s.as_str()).collect();
    let logo: &[&str] = if configured.is_empty() {
        crate::render::START_LOGO
    } else {
        &configured
    };
    // A logo wider than the terminal wraps into noise; the line alone still reads.
    let cols = crossterm::terminal::size().map_or(usize::MAX, |(c, _)| c as usize);
    let logo_fits = logo.iter().all(|l| l.chars().count() <= cols);
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos() as u64);
    let mut out = io::stdout();
    let _ = write(&mut out, config, logo_fits.then_some(logo), message(seed));
}

fn write(
    out: &mut impl Write,
    config: &Config,
    logo: Option<&[&str]>,
    message: &str,
) -> io::Result<()> {
    if let Some(logo) = logo {
        queue!(out, SetForegroundColor(config.theme_info()))?;
        for line in logo {
            queue!(out, Print(line), Print("\n"))?;
        }
        queue!(out, ResetColor, Print("\n"))?;
    }
    queue!(
        out,
        SetForegroundColor(config.theme_dim()),
        Print(message),
        ResetColor,
        Print("\n"),
    )?;
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_seed_picks_a_message() {
        for seed in [0, 1, MESSAGES.len() as u64, u64::MAX] {
            assert!(MESSAGES.contains(&message(seed)));
        }
    }

    #[test]
    fn writes_the_logo_then_the_message() {
        let config = Config::default();
        let mut out = Vec::new();
        write(&mut out, &config, Some(&["AB", "CD"]), "bye").unwrap();
        let text = String::from_utf8(out).unwrap();
        let ab = text.find("AB").unwrap();
        let cd = text.find("CD").unwrap();
        let bye = text.find("bye").unwrap();
        assert!(ab < cd && cd < bye);
    }

    #[test]
    fn writes_only_the_message_without_a_logo() {
        let config = Config::default();
        let mut out = Vec::new();
        write(&mut out, &config, None, "bye").unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("bye") && !text.contains('█'));
    }
}
