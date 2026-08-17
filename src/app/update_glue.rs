//! Startup update check — spawn, drain, surface.
//!
//! The check itself ([`crate::update::check`]) blocks on `curl`, so it runs on
//! its own thread and reports back through a channel like every other
//! background op. Two surfaces consume the result: a one-shot notification and
//! a line under the start-page logo, the latter reading `update.available`
//! directly so it survives the notification's 10s timeout.

use std::thread;

use super::state::UpdateEvent;

impl super::App {
    /// Kick off the check for the session. No-op when `[update] check = false`.
    pub(super) fn update_spawn_check(&self) {
        if !self.config.update.check {
            return;
        }
        let tx = self.update.tx.clone();
        thread::spawn(move || {
            let latest = crate::update::check().ok().flatten();
            let _ = tx.send(UpdateEvent(latest));
        });
    }

    /// Drain the check's result and flush its notice once the status line is
    /// free. Returns `true` if anything changed and the frame needs a repaint.
    pub(super) fn handle_update_events(&mut self) -> bool {
        let mut progress = false;
        while let Ok(UpdateEvent(latest)) = self.update.rx.try_recv() {
            let Some(latest) = latest else {
                continue;
            };
            self.update.pending_notice = Some(format!(
                "update available — binvim {latest} (you have {}) · see :health",
                crate::update::current()
            ));
            self.update.available = Some(latest);
            progress = true;
        }
        if self.status_msg.is_empty()
            && let Some(notice) = self.update.pending_notice.take()
        {
            self.status_msg = notice;
            progress = true;
        }
        progress
    }
}
