//! `:config` — open `config.toml`, and apply it again without a restart,
//! either on `:config reload` or whenever the file is written from binvim.

use crate::config::Config;

impl super::App {
    pub(super) fn config_open(&mut self) {
        let Some(path) = crate::config::config_path() else {
            self.status_msg = "config: no config directory on this system".into();
            return;
        };
        // The buffer for a missing file opens fine, but `:w` can't create
        // the directory it lives in.
        if let Some(dir) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(dir) {
                self.status_msg = format!("config: creating {}: {e}", dir.display());
                return;
            }
        }
        if let Err(e) = self.open_buffer(path) {
            self.status_msg = format!("error: {e}");
        }
    }

    /// Every setting at its default, in a scratch buffer to copy from.
    pub(super) fn config_show_defaults(&mut self) {
        if let Err(e) = self.open_empty_buffer() {
            self.status_msg = format!("error: {e}");
            return;
        }
        self.buffer.insert_str(0, 0, crate::config::DEFAULT_CONFIG);
        self.buffer.dirty = false;
        self.buffer.display_name = Some("[config defaults]".into());
    }

    /// Whether the active buffer is `config.toml`. Compared through symlinks,
    /// since a dotfiles setup usually links the file in from a repo and the
    /// buffer may have been opened from either end.
    pub(super) fn active_is_config(&self) -> bool {
        let (Some(buffer), Some(config)) =
            (self.buffer.path.as_deref(), crate::config::config_path())
        else {
            return false;
        };
        match (
            std::fs::canonicalize(buffer),
            std::fs::canonicalize(&config),
        ) {
            (Ok(a), Ok(b)) => a == b,
            _ => buffer == config,
        }
    }

    /// Re-read `config.toml` and apply it. Returns the message to show.
    pub(super) fn reload_config(&mut self) -> String {
        let text = crate::config::config_path().and_then(|p| std::fs::read_to_string(p).ok());
        self.apply_config_text(text.as_deref())
    }

    /// Apply config text, `None` meaning there's no file. Returns the message
    /// to show.
    fn apply_config_text(&mut self, text: Option<&str>) -> String {
        let config = match text.map(Config::parse) {
            None => Config::default(),
            Some(Ok(config)) => config,
            // Unlike at startup, there's a working config to keep: an edit
            // saved half-finished must not strip the theme mid-session.
            Some(Err(e)) => return format!("{e} — config not reloaded"),
        };
        // The Copilot client is attached per buffer from the first didOpen
        // on, so the flag on the manager only changes at startup.
        let copilot_changed = config.copilot.enabled != self.lsp.copilot_enabled;
        self.config = config;
        self.recolour_highlights();
        let mut message = match self.config.problem_summary() {
            Some(summary) => format!("config reloaded — {summary}"),
            None => "config reloaded".to_string(),
        };
        if copilot_changed {
            message.push_str(" — restart to apply [copilot] enabled");
        }
        message
    }

    /// Highlight caches hold colours already resolved against the old
    /// `[colors]`. The active buffer's is rebuilt on the next render; a
    /// stashed buffer's is rebuilt now, because nothing refreshes it until it's
    /// focused and a split may be showing it.
    fn recolour_highlights(&mut self) {
        self.highlight_cache = None;
        for (i, stash) in self.buffers.iter_mut().enumerate() {
            // The active slot is a stale snapshot; `load_stash` never reads
            // it back.
            if i == self.active || stash.highlight_cache.is_none() {
                continue;
            }
            let lang = stash
                .buffer
                .path
                .as_deref()
                .and_then(crate::lang::Lang::detect);
            stash.highlight_cache =
                lang.and_then(|l| crate::lang::compute_highlights(l, &stash.buffer, &self.config));
        }
    }
}

#[cfg(test)]
mod tests {
    fn app() -> crate::app::App {
        let mut app = crate::app::App::new(None).expect("App::new");
        app.config = crate::config::Config::default();
        app.lsp.copilot_enabled = false;
        app
    }

    #[test]
    fn a_good_config_replaces_the_running_one() {
        let mut app = app();
        let message = app.apply_config_text(Some("[whitespace]\nshow = false"));
        assert_eq!(message, "config reloaded");
        assert!(!app.config.whitespace.show);
    }

    #[test]
    fn a_missing_file_reloads_the_defaults() {
        let mut app = app();
        app.config.whitespace.show = false;
        assert_eq!(app.apply_config_text(None), "config reloaded");
        assert!(app.config.whitespace.show);
    }

    #[test]
    fn a_syntax_error_keeps_the_running_config() {
        let mut app = app();
        app.apply_config_text(Some("[colors]\nbackground = \"#101010\""));
        let message = app.apply_config_text(Some("[colors\nbackground = \"#202020\""));
        assert!(message.ends_with("config not reloaded"), "{message}");
        assert_eq!(
            app.config.colors.get("background").map(String::as_str),
            Some("#101010")
        );
    }

    #[test]
    fn problems_and_a_copilot_change_are_reported() {
        let mut app = app();
        let message = app.apply_config_text(Some("[copilot]\nenabled = true\n[hover]\nwrap = 1"));
        assert!(
            message.starts_with("config reloaded — [hover] wrap:"),
            "{message}"
        );
        assert!(
            message.ends_with("restart to apply [copilot] enabled"),
            "{message}"
        );
        assert!(
            !app.lsp.copilot_enabled,
            "the manager's flag only changes at startup"
        );
    }

    /// The highlight cache stores resolved colours, so a reload that only
    /// swapped the config would leave the old keyword colour on screen.
    #[test]
    fn a_reload_recolours_the_active_buffer() {
        let mut app = app();
        app.buffer =
            crate::buffer::Buffer::from_path("/nonexistent/binvim-test/main.rs".into()).unwrap();
        app.buffer.replace_all("fn main() {}\n");
        app.ensure_highlights();
        app.apply_config_text(Some("[colors]\nkeyword = \"#123456\""));
        app.ensure_highlights();
        let cache = app.highlight_cache.as_ref().expect("highlights rebuilt");
        assert_eq!(
            cache.byte_colors[0],
            Some(crossterm::style::Color::Rgb {
                r: 0x12,
                g: 0x34,
                b: 0x56
            })
        );
    }
}
