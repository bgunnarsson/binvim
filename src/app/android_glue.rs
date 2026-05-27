//! `<leader>A` Android emulator manager — the `App`-side glue over
//! `crate::android`. Lists / launches / creates AVDs and lists running devices,
//! draining results from background worker threads on an mpsc channel (the same
//! pattern as `package_glue` / `lsp` / `dap`). The debug-session entry point
//! lives in `app/dap_glue.rs`'s Android resolver; this file owns the
//! emulator-management flows.
//!
//! Each step that needs a (potentially slow) `sdkmanager` / `avdmanager` / `adb`
//! call opens its picker immediately in a `(loading…)` state and repopulates
//! when the result arrives, mirroring the package-manager flow.

use std::thread;

use crate::android;
use crate::app::state::{AndroidEvent, AndroidFlow};
use crate::mode::{Mode, PromptKind};
use crate::picker::{self, PickerKind, PickerPayload, PickerState};

/// Default hardware profile for newly-created AVDs (the "minimal" create flow
/// exposes only image + name; the device profile is fixed).
const DEFAULT_DEVICE: &str = "pixel";

/// Local TCP port forwarded to the debuggee's JDWP channel and passed to the
/// java-debug adapter's `attach` request.
const ANDROID_JDWP_PORT: u16 = 8000;

impl super::App {
    // ── Entry points (from the `<leader>A` dispatch in dispatch.rs) ──────────

    /// `<leader>Al` — open the AVD picker and list defined emulators.
    pub(super) fn android_list_avds(&mut self) {
        if android::tool_path(android::SdkTool::Emulator).is_none() {
            self.status_msg =
                "android: emulator not found — install the Android SDK (`:install` → Android SDK)"
                    .into();
            return;
        }
        self.android.epoch += 1;
        self.android.flow = None;
        self.picker = Some(PickerState::new(
            PickerKind::AndroidAvd,
            "Android — launch emulator (loading…)".into(),
            Vec::new(),
        ));
        self.mode = Mode::Picker;
        self.android_spawn_list_avds();
    }

    /// `<leader>Ad` — open the running-devices picker and list them.
    pub(super) fn android_list_devices(&mut self) {
        if android::tool_path(android::SdkTool::Adb).is_none() {
            self.status_msg =
                "android: adb not found — install the Android SDK (`:install` → Android SDK)"
                    .into();
            return;
        }
        self.android.epoch += 1;
        self.android.flow = None;
        self.picker = Some(PickerState::new(
            PickerKind::AndroidDevice,
            "Android — running devices (loading…)".into(),
            Vec::new(),
        ));
        self.mode = Mode::Picker;
        self.android_spawn_list_devices();
    }

    /// `<leader>Ac` — start the create-AVD flow: pick a system image, then name
    /// it. Opens the image picker in a loading state while `sdkmanager` runs.
    pub(super) fn android_create_avd(&mut self) {
        if android::tool_path(android::SdkTool::SdkManager).is_none() {
            self.status_msg = "android: sdkmanager not found — install the Android SDK (`:install` → Android SDK)".into();
            return;
        }
        self.android.epoch += 1;
        self.android.flow = Some(AndroidFlow {
            pending_image: None,
        });
        self.picker = Some(PickerState::new(
            PickerKind::AndroidSystemImage,
            "Android — system image (loading…)".into(),
            Vec::new(),
        ));
        self.mode = Mode::Picker;
        self.android_spawn_list_images();
    }

    /// `<leader>Ab` — start an Android debug session. Resolves the Gradle
    /// project, then runs the build → install → `am start -D` → adb JDWP
    /// forward chain on a background thread. On success
    /// (`AndroidEvent::DebugReady`) the flow continues into the jdtls
    /// java-debug bridge and the DAP TCP attach.
    pub(super) fn android_debug_session(&mut self) {
        if android::tool_path(android::SdkTool::Adb).is_none() {
            self.status_msg =
                "android: adb not found — install the Android SDK (`:install` → Android SDK)"
                    .into();
            return;
        }
        let start_dir = self
            .buffer
            .path
            .as_ref()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| ".".into()));
        let Some(root) = android::find_gradle_root(&start_dir) else {
            self.status_msg =
                "android: no Gradle project here (need gradlew / settings.gradle)".into();
            return;
        };
        let jar_present = crate::lsp::java_debug_dir()
            .map(|d| {
                std::fs::read_dir(&d)
                    .map(|mut e| e.next().is_some())
                    .unwrap_or(false)
            })
            .unwrap_or(false);
        if !jar_present {
            self.status_msg = "android: java-debug plugin missing — install it (`:install` → Android SDK) so jdtls can debug".into();
            return;
        }
        self.android.epoch += 1;
        self.android.flow = None;
        self.android_spawn_debug_prep(root);
    }

    // ── Picker-selection handlers (called from picker_glue's Enter dispatch) ─

    /// An AVD was picked from the launch picker — spawn the emulator detached.
    pub(super) fn android_launch_avd(&mut self, name: String) {
        match android::launch_emulator(&name) {
            Ok(()) => self.status_msg = format!("android: launching {name}…"),
            Err(e) => self.status_msg = format!("android: {e}"),
        }
    }

    /// A system image was picked in the create flow — stash it and open the
    /// AVD-name prompt.
    pub(super) fn android_pick_system_image(&mut self, pkg: String) {
        let Some(flow) = self.android.flow.as_mut() else {
            self.status_msg = "android: lost create-AVD context".into();
            return;
        };
        flow.pending_image = Some(pkg);
        self.cmdline.clear();
        self.cmdline_cursor = 0;
        self.mode = Mode::Prompt(PromptKind::AndroidAvdName);
    }

    /// A running device was picked — informational for now (the debug flow
    /// reuses the device list to choose an attach target).
    pub(super) fn android_pick_device(&mut self, serial: String) {
        self.status_msg = format!("android: {serial}");
    }

    /// The AVD-name prompt committed — kick off image-install (idempotent) +
    /// `avdmanager create avd` on a background thread.
    pub(super) fn android_avd_name_entered(&mut self, raw_name: String) {
        // AVD names can't contain whitespace; fold runs of it to underscores.
        // `split_whitespace` already drops leading/trailing runs.
        let name: String = raw_name.split_whitespace().collect::<Vec<_>>().join("_");
        if name.is_empty() {
            self.status_msg = "android: AVD name cannot be empty".into();
            self.android.flow = None;
            return;
        }
        let Some(image) = self
            .android
            .flow
            .as_ref()
            .and_then(|f| f.pending_image.clone())
        else {
            self.status_msg = "android: lost create-AVD context".into();
            return;
        };
        self.android.flow = None;
        self.android_spawn_create(name, image);
    }

    // ── Background spawns ───────────────────────────────────────────────────

    fn android_spawn_list_avds(&mut self) {
        let (tx, epoch) = (self.android.tx.clone(), self.android.epoch);
        self.android.busy = true;
        thread::spawn(move || {
            let result = android::list_avds();
            let _ = tx.send(AndroidEvent::AvdsListed { epoch, result });
        });
    }

    fn android_spawn_list_devices(&mut self) {
        let (tx, epoch) = (self.android.tx.clone(), self.android.epoch);
        self.android.busy = true;
        thread::spawn(move || {
            let result = android::list_running_devices();
            let _ = tx.send(AndroidEvent::DevicesListed { epoch, result });
        });
    }

    fn android_spawn_list_images(&mut self) {
        let (tx, epoch) = (self.android.tx.clone(), self.android.epoch);
        self.android.busy = true;
        thread::spawn(move || {
            let result = android::list_system_images();
            let _ = tx.send(AndroidEvent::ImagesListed { epoch, result });
        });
    }

    fn android_spawn_debug_prep(&mut self, root: std::path::PathBuf) {
        let (tx, epoch) = (self.android.tx.clone(), self.android.epoch);
        self.android.busy = true;
        self.status_msg = "android: building + installing debug app (gradle installDebug)…".into();
        thread::spawn(move || {
            let result = android::prepare_debug(&root, ANDROID_JDWP_PORT);
            let _ = tx.send(AndroidEvent::DebugReady { epoch, result });
        });
    }

    fn android_spawn_create(&mut self, name: String, image: String) {
        let (tx, epoch) = (self.android.tx.clone(), self.android.epoch);
        self.android.busy = true;
        self.status_msg = format!("android: creating {name} (downloading image if needed)…");
        thread::spawn(move || {
            // `sdkmanager <pkg>` is idempotent — a fast no-op when the image is
            // already present, a download otherwise — so always run it first.
            let result = android::install_system_image(&image)
                .and_then(|()| android::create_avd(&name, &image, DEFAULT_DEVICE));
            let _ = tx.send(AndroidEvent::AvdCreated {
                epoch,
                name,
                result,
            });
        });
    }

    // ── Main-loop hook ──────────────────────────────────────────────────────

    /// Drain results from background Android threads. Returns `true` if any
    /// event was processed (so the loop schedules a redraw).
    pub(super) fn handle_android_events(&mut self) -> bool {
        let mut progress = false;
        while let Ok(ev) = self.android.rx.try_recv() {
            progress = true;
            self.android.busy = false;
            let ev_epoch = match &ev {
                AndroidEvent::AvdsListed { epoch, .. }
                | AndroidEvent::DevicesListed { epoch, .. }
                | AndroidEvent::ImagesListed { epoch, .. }
                | AndroidEvent::AvdCreated { epoch, .. }
                | AndroidEvent::DebugReady { epoch, .. } => *epoch,
            };
            // Drop results from a superseded / cancelled flow.
            if ev_epoch != self.android.epoch {
                continue;
            }
            match ev {
                AndroidEvent::AvdsListed { result, .. } => self.android_on_avds(result),
                AndroidEvent::DevicesListed { result, .. } => self.android_on_devices(result),
                AndroidEvent::ImagesListed { result, .. } => self.android_on_images(result),
                AndroidEvent::AvdCreated { name, result, .. } => {
                    self.status_msg = match result {
                        Ok(()) => format!("android: created {name} — <leader>Al to launch"),
                        Err(e) => format!("android: {e}"),
                    };
                }
                AndroidEvent::DebugReady { result, .. } => self.android_on_debug_ready(result),
            }
        }
        progress
    }

    // ── Result handlers ─────────────────────────────────────────────────────

    fn android_on_avds(&mut self, result: Result<Vec<String>, String>) {
        if !matches!(
            self.picker.as_ref().map(|p| p.kind),
            Some(PickerKind::AndroidAvd)
        ) {
            return;
        }
        match result {
            Err(e) => self.android_abort(format!("android: {e}")),
            Ok(names) if names.is_empty() => {
                self.android_abort("android: no AVDs — <leader>Ac to create one".into())
            }
            Ok(names) => {
                let items: Vec<(String, PickerPayload)> = names
                    .into_iter()
                    .map(|name| (name.clone(), PickerPayload::AndroidAvd { name }))
                    .collect();
                if let Some(picker) = self.picker.as_mut() {
                    picker.title = "Android — launch emulator".into();
                    picker::replace_items(picker, items);
                }
            }
        }
    }

    fn android_on_devices(&mut self, result: Result<Vec<android::Device>, String>) {
        if !matches!(
            self.picker.as_ref().map(|p| p.kind),
            Some(PickerKind::AndroidDevice)
        ) {
            return;
        }
        match result {
            Err(e) => self.android_abort(format!("android: {e}")),
            Ok(devices) if devices.is_empty() => {
                self.android_abort("android: no devices — <leader>Al to launch an emulator".into())
            }
            Ok(devices) => {
                let items: Vec<(String, PickerPayload)> = devices
                    .into_iter()
                    .map(|d| {
                        let model = d.model.as_deref().unwrap_or("");
                        let display = format!("{}  {}  {}", d.serial, d.state, model);
                        (
                            display.trim_end().to_string(),
                            PickerPayload::AndroidDevice { serial: d.serial },
                        )
                    })
                    .collect();
                if let Some(picker) = self.picker.as_mut() {
                    picker.title = "Android — running devices".into();
                    picker::replace_items(picker, items);
                }
            }
        }
    }

    fn android_on_images(&mut self, result: Result<Vec<android::SystemImage>, String>) {
        if !matches!(
            self.picker.as_ref().map(|p| p.kind),
            Some(PickerKind::AndroidSystemImage)
        ) {
            return;
        }
        match result {
            Err(e) => self.android_abort(format!("android: {e}")),
            Ok(images) if images.is_empty() => {
                self.android_abort("android: no system images offered by sdkmanager".into())
            }
            Ok(images) => {
                // Installed images sort first (parser guarantees) and are flagged
                // with a marker; selecting an un-installed one downloads it.
                let mut marked: Option<usize> = None;
                let items: Vec<(String, PickerPayload)> = images
                    .into_iter()
                    .enumerate()
                    .map(|(i, img)| {
                        let display = if img.installed {
                            if marked.is_none() {
                                marked = Some(i);
                            }
                            format!("{}  ● installed", img.pkg)
                        } else {
                            img.pkg.clone()
                        };
                        (display, PickerPayload::AndroidSystemImage { pkg: img.pkg })
                    })
                    .collect();
                if let Some(picker) = self.picker.as_mut() {
                    picker.title = "Android — system image (Enter to pick)".into();
                    picker::replace_items(picker, items);
                    picker.marked = marked;
                }
            }
        }
    }

    /// jdtls handed back the java-debug DAP port — connect a DAP attach
    /// session to it, pointed at the adb-forwarded JDWP port. Called from
    /// `lsp_glue`'s `LspEvent::JavaDebugSession` handler.
    pub(super) fn android_attach_debug(&mut self, dap_port: u16) {
        let Some(prep) = self.pending_android_debug.take() else {
            self.status_msg = "android: debug session lost context".into();
            return;
        };
        let project_name = prep
            .project_root
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let attach_args = serde_json::json!({
            "type": "java",
            "request": "attach",
            "hostName": "127.0.0.1",
            "port": prep.local_jdwp_port,
            "projectName": project_name,
        });
        match self.dap.start_attach_session(
            "android",
            "java",
            dap_port,
            prep.project_root,
            attach_args,
        ) {
            Ok(()) => self.status_msg = format!("android: attaching to {}…", prep.package),
            Err(e) => self.status_msg = format!("android: {e}"),
        }
    }

    /// adb prep finished. On success, stash the context and ask jdtls for a
    /// java-debug DAP port; the attach happens when that reply lands
    /// (`LspEvent::JavaDebugSession`, handled in `lsp_glue`).
    fn android_on_debug_ready(&mut self, result: Result<android::DebugPrep, String>) {
        match result {
            Err(e) => {
                self.pending_android_debug = None;
                self.status_msg = format!("android: {e}");
            }
            Ok(prep) => {
                self.pending_android_debug = Some(prep);
                match self.lsp.request_java_debug_session() {
                    Ok(()) => {
                        self.status_msg = "android: app running — starting debug session…".into()
                    }
                    Err(e) => {
                        self.pending_android_debug = None;
                        self.status_msg = format!("android: {e}");
                    }
                }
            }
        }
    }

    /// Tear down the picker + flow and surface `msg` on the status line.
    fn android_abort(&mut self, msg: String) {
        self.picker = None;
        self.mode = Mode::Normal;
        self.android.flow = None;
        self.status_msg = msg;
    }
}
