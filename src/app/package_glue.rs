//! `<leader>p` package-manager flow — ecosystem detection, the install /
//! search state machine, and the background-thread plumbing. Pure CLI + parse
//! logic lives in `crate::package`; this file is the `App`-side glue: it chains
//! the manifest → package/search → version pickers and drains the mpsc channel
//! the worker threads post results on (same pattern as `lsp` / `dap` / `test`).
//!
//! Each step that needs a fetch opens its picker immediately in a `(loading…)`
//! state and repopulates when the result arrives — mirroring how the
//! `WorkspaceSymbols` picker streams in async results.

use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use crate::app::state::{PackageEvent, PackageFlow, PackageFlowKind};
use crate::mode::Mode;
use crate::package::{self, PackageEcosystem, PackageVersion};
use crate::picker::{self, PickerKind, PickerPayload, PickerState};

/// Debounce before a registry search fires, so each keystroke in the search
/// picker doesn't spawn its own network request.
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(350);
/// Minimum query length before searching the registry.
const SEARCH_MIN_LEN: usize = 3;

impl super::App {
    /// Entry point for `<leader>pi` / `<leader>ps`. Detects the ecosystem,
    /// discovers manifests, and either auto-selects the lone manifest or opens
    /// the manifest picker. Mirrors `dap_resolve_dotnet`.
    pub(super) fn package_begin(&mut self, kind: PackageFlowKind) {
        let buffer_path = self.buffer.path.clone();
        let start_dir = buffer_path
            .as_ref()
            .and_then(|p| p.parent().map(|p| p.to_path_buf()))
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        // Today every backend is .NET, so the .NET-flavoured root walk is the
        // right one; when npm / cargo land this moves behind `detect`.
        let workspace_root = crate::dap::find_dotnet_workspace_root(&start_dir);
        let Some(eco) = package::detect(buffer_path.as_deref(), &workspace_root) else {
            self.status_msg = "No package manager detected for this buffer".into();
            return;
        };
        let manifests = package::find_manifests(eco, &workspace_root);
        if manifests.is_empty() {
            self.status_msg = format!("{}: no project manifest found", eco.label());
            return;
        }
        // New flow — bump the epoch so any late result from a prior flow that
        // wasn't cleanly cancelled is dropped on arrival.
        self.package.epoch += 1;
        self.package.flow = Some(PackageFlow {
            eco,
            kind,
            manifest: None,
            package_id: None,
            include_prerelease: false,
            version_cache: Vec::new(),
            installed_version: String::new(),
            search_dirty_at: None,
        });
        if manifests.len() == 1 {
            let path = manifests.into_iter().next().unwrap().path;
            self.pkg_manifest_chosen(path);
        } else {
            let items = manifests
                .into_iter()
                .map(|m| (m.display, PickerPayload::PackageManifest(m.path)))
                .collect();
            let title = format!("{} — project", eco.label());
            self.picker = Some(PickerState::new(PickerKind::PackageManifest, title, items));
            self.mode = Mode::Picker;
        }
    }

    /// Manifest picked (or auto-selected) — stash it and move to step 2:
    /// list installed packages (Install) or open the search picker (Search).
    fn pkg_manifest_chosen(&mut self, manifest: PathBuf) {
        let Some(flow) = self.package.flow.as_mut() else {
            return;
        };
        flow.manifest = Some(manifest.clone());
        let eco = flow.eco;
        match flow.kind {
            PackageFlowKind::Install => {
                let title = format!("{} — installed (loading…)", eco.label());
                self.picker = Some(PickerState::new(
                    PickerKind::PackageInstalled,
                    title,
                    Vec::new(),
                ));
                self.mode = Mode::Picker;
                self.pkg_spawn_installed(manifest);
            }
            PackageFlowKind::Search => {
                let title = format!("{} — search (type ≥{SEARCH_MIN_LEN} chars)", eco.label());
                self.picker = Some(PickerState::new(
                    PickerKind::PackageSearch,
                    title,
                    Vec::new(),
                ));
                self.mode = Mode::Picker;
            }
        }
    }

    // ── Picker-selection handlers (called from picker_glue's Enter dispatch) ─

    pub(super) fn pkg_pick_manifest(&mut self, path: PathBuf) {
        self.pkg_manifest_chosen(path);
    }

    pub(super) fn pkg_pick_installed(&mut self, id: String, installed: String) {
        let Some(flow) = self.package.flow.as_mut() else {
            return;
        };
        flow.package_id = Some(id.clone());
        flow.installed_version = installed;
        flow.include_prerelease = false;
        self.pkg_open_version_picker(id);
    }

    pub(super) fn pkg_pick_search_hit(&mut self, id: String) {
        let Some(flow) = self.package.flow.as_mut() else {
            return;
        };
        flow.package_id = Some(id.clone());
        flow.installed_version = String::new(); // brand-new package, nothing installed
        flow.include_prerelease = false;
        self.pkg_open_version_picker(id);
    }

    pub(super) fn pkg_pick_version(&mut self, version: String) {
        let Some(flow) = self.package.flow.as_ref() else {
            return;
        };
        let (Some(manifest), Some(id), eco) =
            (flow.manifest.clone(), flow.package_id.clone(), flow.eco)
        else {
            self.status_msg = "package: lost flow context".into();
            self.package.flow = None;
            return;
        };
        // The picker was already torn down by the Enter handler; kick off the
        // install and end the flow.
        self.package.flow = None;
        self.pkg_spawn_add(eco, manifest, id, version);
    }

    /// `Tab` in the version picker — flip prerelease visibility and rebuild
    /// the row set from the cached version list (no refetch).
    pub(super) fn pkg_toggle_prerelease(&mut self) {
        if let Some(flow) = self.package.flow.as_mut() {
            flow.include_prerelease = !flow.include_prerelease;
        }
        self.pkg_rebuild_version_picker();
    }

    /// Record that the search picker's query changed; `pkg_search_tick` fires
    /// the debounced registry search from the main loop.
    pub(super) fn pkg_mark_search_dirty(&mut self) {
        if let Some(flow) = self.package.flow.as_mut() {
            flow.search_dirty_at = Some(Instant::now());
        }
    }

    fn pkg_open_version_picker(&mut self, id: String) {
        let eco = self.pkg_eco();
        let title = format!("{} — {id} versions (loading…)", eco.label());
        self.picker = Some(PickerState::new(
            PickerKind::PackageVersion,
            title,
            Vec::new(),
        ));
        self.mode = Mode::Picker;
        self.pkg_spawn_versions(id);
    }

    // ── Background spawns ───────────────────────────────────────────────────

    fn pkg_spawn_installed(&mut self, manifest: PathBuf) {
        let (eco, tx, epoch) = (self.pkg_eco(), self.package.tx.clone(), self.package.epoch);
        self.package.busy = true;
        thread::spawn(move || {
            let result = package::list_installed(eco, &manifest);
            let _ = tx.send(PackageEvent::Installed { epoch, result });
        });
    }

    fn pkg_spawn_versions(&mut self, id: String) {
        let (eco, tx, epoch) = (self.pkg_eco(), self.package.tx.clone(), self.package.epoch);
        self.package.busy = true;
        thread::spawn(move || {
            let result = package::list_versions(eco, &id);
            let _ = tx.send(PackageEvent::Versions { epoch, result });
        });
    }

    fn pkg_spawn_search(&mut self, query: String) {
        let (eco, tx, epoch) = (self.pkg_eco(), self.package.tx.clone(), self.package.epoch);
        self.package.busy = true;
        thread::spawn(move || {
            let result = package::search(eco, &query);
            let _ = tx.send(PackageEvent::SearchResults {
                epoch,
                query,
                result,
            });
        });
    }

    fn pkg_spawn_add(
        &mut self,
        eco: PackageEcosystem,
        manifest: PathBuf,
        id: String,
        version: String,
    ) {
        let (tx, epoch) = (self.package.tx.clone(), self.package.epoch);
        self.package.busy = true;
        self.status_msg = format!("{}: adding {id} {version}…", eco.label());
        thread::spawn(move || {
            let result = package::add(eco, &manifest, &id, &version);
            let _ = tx.send(PackageEvent::AddDone {
                epoch,
                id,
                version,
                result,
            });
        });
    }

    fn pkg_eco(&self) -> PackageEcosystem {
        self.package
            .flow
            .as_ref()
            .map(|f| f.eco)
            .unwrap_or(PackageEcosystem::DotNet)
    }

    // ── Main-loop hooks ─────────────────────────────────────────────────────

    /// Drain results from background package threads. Returns `true` if any
    /// event was processed (so the loop schedules a redraw).
    pub(super) fn handle_package_events(&mut self) -> bool {
        let mut progress = false;
        while let Ok(ev) = self.package.rx.try_recv() {
            progress = true;
            self.package.busy = false;
            let ev_epoch = match &ev {
                PackageEvent::Installed { epoch, .. }
                | PackageEvent::Versions { epoch, .. }
                | PackageEvent::SearchResults { epoch, .. }
                | PackageEvent::AddDone { epoch, .. } => *epoch,
            };
            // Drop results from a superseded / cancelled flow.
            if ev_epoch != self.package.epoch {
                continue;
            }
            match ev {
                PackageEvent::Installed { result, .. } => self.pkg_on_installed(result),
                PackageEvent::Versions { result, .. } => self.pkg_on_versions(result),
                PackageEvent::SearchResults { query, result, .. } => {
                    self.pkg_on_search(query, result)
                }
                PackageEvent::AddDone {
                    id,
                    version,
                    result,
                    ..
                } => {
                    self.status_msg = match result {
                        Ok(()) => format!("Added {id} {version}"),
                        Err(e) => format!("package: {e}"),
                    };
                }
            }
        }
        progress
    }

    /// Fire the debounced registry search once the search picker's query has
    /// settled. Returns `true` if a search was kicked off.
    pub(super) fn pkg_search_tick(&mut self) -> bool {
        let due = match self.package.flow.as_ref() {
            Some(flow) if matches!(flow.kind, PackageFlowKind::Search) => flow
                .search_dirty_at
                .is_some_and(|t| Instant::now() >= t + SEARCH_DEBOUNCE),
            _ => false,
        };
        if !due {
            return false;
        }
        if let Some(flow) = self.package.flow.as_mut() {
            flow.search_dirty_at = None;
        }
        let query = self
            .picker
            .as_ref()
            .map(|p| p.input.clone())
            .unwrap_or_default();
        if query.len() >= SEARCH_MIN_LEN {
            self.pkg_spawn_search(query);
            true
        } else {
            false
        }
    }

    // ── Result handlers ─────────────────────────────────────────────────────

    fn pkg_on_installed(&mut self, result: Result<Vec<package::InstalledPackage>, String>) {
        if !matches!(
            self.picker.as_ref().map(|p| p.kind),
            Some(PickerKind::PackageInstalled)
        ) {
            return;
        }
        match result {
            Err(e) => self.pkg_abort(format!("package: {e}")),
            Ok(pkgs) if pkgs.is_empty() => {
                self.pkg_abort("No packages installed — use <leader>ps to add one".into())
            }
            Ok(pkgs) => {
                let eco = self.pkg_eco();
                let items: Vec<(String, PickerPayload)> = pkgs
                    .into_iter()
                    .map(|p| {
                        let display = format!("{}  {}", p.id, p.resolved);
                        (
                            display,
                            PickerPayload::PackageInstalled {
                                id: p.id,
                                installed: p.resolved,
                            },
                        )
                    })
                    .collect();
                if let Some(picker) = self.picker.as_mut() {
                    picker.title = format!("{} — installed", eco.label());
                    picker::replace_items(picker, items);
                }
            }
        }
    }

    fn pkg_on_versions(&mut self, result: Result<Vec<PackageVersion>, String>) {
        if !matches!(
            self.picker.as_ref().map(|p| p.kind),
            Some(PickerKind::PackageVersion)
        ) {
            return;
        }
        match result {
            Err(e) => self.pkg_abort(format!("package: {e}")),
            Ok(versions) if versions.is_empty() => self.pkg_abort("No versions found".into()),
            Ok(versions) => {
                if let Some(flow) = self.package.flow.as_mut() {
                    flow.version_cache = versions;
                }
                self.pkg_rebuild_version_picker();
            }
        }
    }

    fn pkg_on_search(&mut self, query: String, result: Result<Vec<package::SearchHit>, String>) {
        // Apply only if the search picker is still open AND its query hasn't
        // moved on (a slower earlier search must not clobber a newer one).
        let stale = self
            .picker
            .as_ref()
            .map(|p| p.kind != PickerKind::PackageSearch || p.input != query)
            .unwrap_or(true);
        if stale {
            return;
        }
        match result {
            Err(e) => self.status_msg = format!("package: {e}"),
            Ok(hits) => {
                let items: Vec<(String, PickerPayload)> = hits
                    .into_iter()
                    .map(|h| {
                        let display = format!("{}  {}", h.id, h.latest);
                        (display, PickerPayload::PackageSearchHit { id: h.id })
                    })
                    .collect();
                if let Some(picker) = self.picker.as_mut() {
                    // `replace_items` keeps `input`, and we deliberately don't
                    // re-filter: the registry already matched the query.
                    picker::replace_items(picker, items);
                }
            }
        }
    }

    /// Rebuild the version picker's rows from the cached version list, honouring
    /// the prerelease toggle and flagging the installed version. Preserves the
    /// typed narrowing query across a `Tab` toggle.
    fn pkg_rebuild_version_picker(&mut self) {
        let Some(flow) = self.package.flow.as_ref() else {
            return;
        };
        let eco = flow.eco;
        let id = flow.package_id.clone().unwrap_or_default();
        let installed = flow.installed_version.clone();
        let include_pre = flow.include_prerelease;
        let mut marked: Option<usize> = None;
        let mut items: Vec<(String, PickerPayload)> = Vec::new();
        for v in flow.version_cache.iter() {
            if v.prerelease && !include_pre {
                continue;
            }
            let is_installed = !installed.is_empty() && v.version == installed;
            if is_installed {
                marked = Some(items.len());
            }
            let display = if is_installed {
                format!("{}  ● installed", v.version)
            } else {
                v.version.clone()
            };
            items.push((
                display,
                PickerPayload::PackageVersion {
                    version: v.version.clone(),
                },
            ));
        }
        let pre_note = if include_pre { "stable+pre" } else { "stable" };
        let title = format!(
            "{} — {id} versions [{pre_note}]  (Tab: prerelease)",
            eco.label()
        );
        if let Some(picker) = self.picker.as_mut() {
            picker.title = title;
            picker::replace_items(picker, items); // keeps picker.input
            picker.marked = marked;
            if picker.input.is_empty() {
                if let Some(m) = marked {
                    picker.selected = picker.filtered.iter().position(|&i| i == m).unwrap_or(0);
                }
            } else {
                // Re-apply the user's narrowing query against the new row set.
                picker.refilter();
            }
        }
    }

    /// Tear down the flow + picker and surface `msg` on the status line.
    fn pkg_abort(&mut self, msg: String) {
        self.picker = None;
        self.mode = Mode::Normal;
        self.package.flow = None;
        self.status_msg = msg;
    }
}
