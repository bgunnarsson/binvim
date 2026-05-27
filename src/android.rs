//! Android SDK command-line backend behind the `<leader>a` entry point —
//! emulator (AVD) management without Android Studio. Everything here is pure
//! CLI invocation + parsing over Google's standalone command-line tools
//! (`sdkmanager` / `avdmanager` / `adb` / `emulator`), so the parsers below
//! carry the test coverage, mirroring `package.rs`.
//!
//! The tools are usually *not* on `$PATH` (only the Homebrew casks link
//! `sdkmanager` / `avdmanager` / `adb`; the emulator and system images live
//! under `$ANDROID_HOME`), so [`tool_path`] resolves each one by probing
//! `$PATH` first and then well-known locations under the SDK root. The
//! `App`-side flow (background-thread channel + picker chaining) lives in
//! `app/android_glue.rs`.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// One of the SDK command-line tools we shell out to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SdkTool {
    SdkManager,
    AvdManager,
    Adb,
    Emulator,
}

impl SdkTool {
    /// The candidate filenames for this tool, including the Windows variants
    /// (the cmdline-tools ship `.bat` shims; adb / emulator ship `.exe`).
    fn candidate_filenames(self) -> &'static [&'static str] {
        match self {
            SdkTool::SdkManager => &["sdkmanager", "sdkmanager.bat"],
            SdkTool::AvdManager => &["avdmanager", "avdmanager.bat"],
            SdkTool::Adb => &["adb", "adb.exe"],
            SdkTool::Emulator => &["emulator", "emulator.exe"],
        }
    }

    /// Directories relative to the SDK root where this tool may live.
    fn sdk_rel_dirs(self) -> &'static [&'static str] {
        match self {
            // `latest` is the modern layout; the bare `bin` and legacy `tools`
            // cover older unzips.
            SdkTool::SdkManager | SdkTool::AvdManager => {
                &["cmdline-tools/latest/bin", "cmdline-tools/bin", "tools/bin"]
            }
            SdkTool::Adb => &["platform-tools"],
            SdkTool::Emulator => &["emulator"],
        }
    }

    fn label(self) -> &'static str {
        match self {
            SdkTool::SdkManager => "sdkmanager",
            SdkTool::AvdManager => "avdmanager",
            SdkTool::Adb => "adb",
            SdkTool::Emulator => "emulator",
        }
    }
}

/// One running device / emulator as reported by `adb devices -l`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub serial: String,
    /// `device`, `offline`, `unauthorized`, …
    pub state: String,
    /// The `model:` field when adb reports it.
    pub model: Option<String>,
}

/// One system image package from `sdkmanager --list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemImage {
    /// The package path, e.g. `system-images;android-34;google_apis;arm64-v8a`.
    pub pkg: String,
    /// Whether it appeared under the "Installed packages" section.
    pub installed: bool,
}

/// Resolve the Android SDK root. Honours `ANDROID_HOME` then
/// `ANDROID_SDK_ROOT`, then falls back to the per-platform default location.
pub fn sdk_root() -> Option<PathBuf> {
    for var in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
        if let Some(v) = std::env::var_os(var) {
            if !v.is_empty() {
                let p = PathBuf::from(v);
                if p.is_dir() {
                    return Some(p);
                }
            }
        }
    }
    let home = crate::paths::home_dir()?;
    let default = if cfg!(target_os = "macos") {
        home.join("Library").join("Android").join("sdk")
    } else if cfg!(windows) {
        // %LOCALAPPDATA%\Android\Sdk, falling back to the home-relative form.
        std::env::var_os("LOCALAPPDATA")
            .map(|l| PathBuf::from(l).join("Android").join("Sdk"))
            .unwrap_or_else(|| {
                home.join("AppData")
                    .join("Local")
                    .join("Android")
                    .join("Sdk")
            })
    } else {
        home.join("Android").join("Sdk")
    };
    default.is_dir().then_some(default)
}

/// Resolve a tool's executable path: `$PATH` first (the Homebrew casks link
/// `sdkmanager` / `avdmanager` / `adb`), then the SDK-root-relative locations.
pub fn tool_path(tool: SdkTool) -> Option<PathBuf> {
    for name in tool.candidate_filenames() {
        if let Some(p) = crate::paths::find_on_path(name) {
            return Some(p);
        }
    }
    let root = sdk_root()?;
    for dir in tool.sdk_rel_dirs() {
        for name in tool.candidate_filenames() {
            let p = root.join(dir).join(name);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

/// Build a `Command` for `tool` with the SDK env vars pre-set so the tool can
/// locate the rest of the SDK regardless of how it was found.
fn sdk_command(tool: SdkTool) -> Result<Command, String> {
    let path = tool_path(tool).ok_or_else(|| {
        format!(
            "{} not found — install the Android SDK (`:install` → Android SDK)",
            tool.label()
        )
    })?;
    let mut cmd = Command::new(path);
    if let Some(root) = sdk_root() {
        cmd.env("ANDROID_SDK_ROOT", &root);
        cmd.env("ANDROID_HOME", &root);
    }
    Ok(cmd)
}

/// Run a command to completion and return stdout, surfacing a trimmed slice of
/// stderr on failure (same shape as `package::run_capture`).
fn run_capture(mut cmd: Command, label: &str) -> Result<String, String> {
    let output = cmd
        .output()
        .map_err(|e| format!("failed to run {label}: {e}"))?;
    if !output.status.success() {
        let pick = |bytes: &[u8]| -> String {
            String::from_utf8_lossy(bytes)
                .lines()
                .map(|l| l.trim())
                .filter(|l| !l.is_empty())
                .take(4)
                .collect::<Vec<_>>()
                .join(" / ")
        };
        let mut msg = pick(&output.stderr);
        if msg.is_empty() {
            msg = pick(&output.stdout);
        }
        if msg.is_empty() {
            msg = "(no output)".to_string();
        }
        return Err(format!("{label}: {msg}"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Spawn a command, feed `stdin_feed` to its stdin, and wait. Used for the
/// prompts `sdkmanager` (license `y`) and `avdmanager` (custom-hardware `no`)
/// raise — there's no flag to suppress them.
fn run_with_stdin(mut cmd: Command, stdin_feed: &str, label: &str) -> Result<(), String> {
    use std::io::Write;
    cmd.stdin(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to run {label}: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(stdin_feed.as_bytes());
        // Dropping stdin closes it so the tool sees EOF after our feed.
    }
    let output = child
        .wait_with_output()
        .map_err(|e| format!("{label} wait failed: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("(no output)");
        return Err(format!("{label}: {detail}"));
    }
    Ok(())
}

/// List the names of all defined AVDs via `emulator -list-avds`.
pub fn list_avds() -> Result<Vec<String>, String> {
    let mut cmd = sdk_command(SdkTool::Emulator)?;
    cmd.arg("-list-avds");
    let out = run_capture(cmd, "emulator -list-avds")?;
    Ok(parse_avd_list(&out))
}

/// List running devices / emulators via `adb devices -l`.
pub fn list_running_devices() -> Result<Vec<Device>, String> {
    let mut cmd = sdk_command(SdkTool::Adb)?;
    cmd.args(["devices", "-l"]);
    let out = run_capture(cmd, "adb devices")?;
    Ok(parse_adb_devices(&out))
}

/// List system images via `sdkmanager --list`. Installed images sort first.
pub fn list_system_images() -> Result<Vec<SystemImage>, String> {
    let mut cmd = sdk_command(SdkTool::SdkManager)?;
    cmd.arg("--list");
    let out = run_capture(cmd, "sdkmanager --list")?;
    Ok(parse_sdkmanager_images(&out))
}

/// Download + install a system image package via `sdkmanager "<pkg>"`,
/// auto-accepting any license prompts.
pub fn install_system_image(pkg: &str) -> Result<(), String> {
    let mut cmd = sdk_command(SdkTool::SdkManager)?;
    cmd.arg(pkg);
    // Each pending license raises an "Accept? (y/N)" prompt; feed a generous
    // run of `y` so they all clear.
    run_with_stdin(cmd, &"y\n".repeat(50), "sdkmanager install")
}

/// Create an AVD via `avdmanager create avd`. `device` is the hardware profile
/// id (e.g. `pixel`). The `--force` overwrites an existing AVD of the same name;
/// `no` answers the custom-hardware-profile prompt.
pub fn create_avd(name: &str, image_pkg: &str, device: &str) -> Result<(), String> {
    let mut cmd = sdk_command(SdkTool::AvdManager)?;
    cmd.args([
        "create", "avd", "--force", "-n", name, "-k", image_pkg, "-d", device,
    ]);
    run_with_stdin(cmd, "no\n", "avdmanager create avd")
}

/// Launch an emulator for `name` as a detached background process. The emulator
/// is a GUI app with its own window — binvim stays in the foreground — so this
/// spawns and returns immediately rather than handing over the terminal. On
/// Unix the child is put in its own process group so a terminal hangup or
/// `Ctrl-C` in binvim doesn't take the emulator down with it.
pub fn launch_emulator(name: &str) -> Result<(), String> {
    let mut cmd = sdk_command(SdkTool::Emulator)?;
    cmd.arg("-avd").arg(name);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // pgid 0 → a fresh process group rooted at the child.
        cmd.process_group(0);
    }
    cmd.spawn()
        .map(|_| ())
        .map_err(|e| format!("failed to launch emulator {name}: {e}"))
}

// ─── debug attach orchestration (JDWP over adb) ─────────────────────────────

/// Everything the DAP attach step needs once the app is built, installed, and
/// running on a device with its JDWP port forwarded.
#[derive(Debug, Clone)]
pub struct DebugPrep {
    pub package: String,
    /// Local TCP port forwarded to the app's JDWP channel.
    pub local_jdwp_port: u16,
    pub project_root: PathBuf,
}

/// Walk up from `start` for the Gradle project root (a `gradlew` wrapper or a
/// `settings.gradle[.kts]`).
pub fn find_gradle_root(start: &Path) -> Option<PathBuf> {
    let markers = [
        "gradlew",
        "settings.gradle",
        "settings.gradle.kts",
        "build.gradle",
        "build.gradle.kts",
    ];
    let mut dir = Some(start);
    while let Some(d) = dir {
        if markers.iter().any(|m| d.join(m).exists()) {
            return Some(d.to_path_buf());
        }
        dir = d.parent();
    }
    None
}

/// Build + install the debug APK, launch it waiting for a debugger, find its
/// JDWP pid, and forward `local_port` to it. Runs on a background thread (it
/// blocks on Gradle + adb), returning everything the DAP attach needs.
pub fn prepare_debug(project_root: &Path, local_port: u16) -> Result<DebugPrep, String> {
    let serial = list_running_devices()?
        .into_iter()
        .find(|d| d.state == "device")
        .map(|d| d.serial)
        .ok_or("no running device — launch an emulator first (<leader>Al)")?;

    gradle_install_debug(project_root)?;

    let pkg =
        read_application_id(project_root).ok_or("could not find applicationId in build.gradle")?;
    let activity = resolve_launch_activity(&serial, &pkg)?;

    // `-D` makes the app halt at startup until a debugger attaches.
    let mut start = adb(&serial)?;
    start.args([
        "shell",
        "am",
        "start",
        "-D",
        "-n",
        &format!("{pkg}/{activity}"),
    ]);
    run_capture(start, "adb am start")?;

    let pid = jdwp_pid(&serial, &pkg)?;
    let mut fwd = adb(&serial)?;
    fwd.args([
        "forward",
        &format!("tcp:{local_port}"),
        &format!("jdwp:{pid}"),
    ]);
    run_capture(fwd, "adb forward")?;

    Ok(DebugPrep {
        package: pkg,
        local_jdwp_port: local_port,
        project_root: project_root.to_path_buf(),
    })
}

/// `adb -s <serial> …` command with the SDK env pre-set.
fn adb(serial: &str) -> Result<Command, String> {
    let mut cmd = sdk_command(SdkTool::Adb)?;
    cmd.args(["-s", serial]);
    Ok(cmd)
}

fn gradle_install_debug(root: &Path) -> Result<(), String> {
    let wrapper = root.join(if cfg!(windows) {
        "gradlew.bat"
    } else {
        "gradlew"
    });
    let mut cmd = if wrapper.is_file() {
        Command::new(wrapper)
    } else {
        Command::new("gradle")
    };
    cmd.current_dir(root);
    cmd.arg("installDebug");
    run_capture(cmd, "gradle installDebug").map(|_| ())
}

fn resolve_launch_activity(serial: &str, pkg: &str) -> Result<String, String> {
    let mut cmd = adb(serial)?;
    cmd.args([
        "shell",
        "cmd",
        "package",
        "resolve-activity",
        "--brief",
        pkg,
    ]);
    let out = run_capture(cmd, "adb resolve-activity")?;
    parse_resolve_activity(&out)
        .ok_or_else(|| format!("could not resolve launch activity for {pkg}"))
}

fn jdwp_pid(serial: &str, pkg: &str) -> Result<String, String> {
    let mut cmd = adb(serial)?;
    cmd.args(["shell", "pidof", pkg]);
    let out = run_capture(cmd, "adb pidof")?;
    out.split_whitespace()
        .next()
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("no running process for {pkg} — did the app start?"))
}

/// Read `applicationId` from the project's Gradle build files. Checks the
/// conventional `app/` module first, then the project root.
fn read_application_id(root: &Path) -> Option<String> {
    for rel in [
        "app/build.gradle.kts",
        "app/build.gradle",
        "build.gradle.kts",
        "build.gradle",
    ] {
        if let Ok(text) = std::fs::read_to_string(root.join(rel)) {
            if let Some(id) = parse_application_id(&text) {
                return Some(id);
            }
        }
    }
    None
}

// ─── parsers (tested) ───────────────────────────────────────────────────────

/// Parse `emulator -list-avds` output. The command prints one AVD name per
/// line, but can interleave `INFO`/warning lines — AVD names are restricted to
/// `[A-Za-z0-9._-]`, so anything with other characters (spaces, colons) is
/// noise and dropped.
pub fn parse_avd_list(out: &str) -> Vec<String> {
    out.lines()
        .map(str::trim)
        .filter(|l| {
            !l.is_empty()
                && l.chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        })
        .map(str::to_string)
        .collect()
}

/// Parse `adb devices -l` output. The first line is the
/// "List of devices attached" header; each subsequent line is
/// `<serial> <state> [key:value ...]`.
pub fn parse_adb_devices(out: &str) -> Vec<Device> {
    let mut devices = Vec::new();
    for line in out.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("List of devices") {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(serial) = parts.next() else { continue };
        let Some(state) = parts.next() else { continue };
        let model = parts
            .find_map(|tok| tok.strip_prefix("model:"))
            .map(str::to_string);
        devices.push(Device {
            serial: serial.to_string(),
            state: state.to_string(),
            model,
        });
    }
    devices
}

/// Parse `sdkmanager --list` output, extracting `system-images;…` packages and
/// whether each appeared under the "Installed packages" section. Rows are
/// pipe-delimited (`path | version | description`). De-duplicates by package
/// path, preferring an installed hit. Installed images sort first, then by path.
pub fn parse_sdkmanager_images(out: &str) -> Vec<SystemImage> {
    // false = "Available", true = "Installed". sdkmanager prints the installed
    // section first; the available section follows its own header.
    let mut in_installed = false;
    let mut seen: Vec<SystemImage> = Vec::new();
    for line in out.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("installed packages") {
            in_installed = true;
            continue;
        }
        if lower.starts_with("available packages") || lower.starts_with("available updates") {
            in_installed = false;
            continue;
        }
        if !trimmed.contains('|') {
            continue;
        }
        let pkg = trimmed.split('|').next().unwrap_or("").trim();
        if !pkg.starts_with("system-images;") {
            continue;
        }
        match seen.iter_mut().find(|s| s.pkg == pkg) {
            Some(existing) => existing.installed |= in_installed,
            None => seen.push(SystemImage {
                pkg: pkg.to_string(),
                installed: in_installed,
            }),
        }
    }
    seen.sort_by(|a, b| {
        b.installed
            .cmp(&a.installed)
            .then_with(|| a.pkg.cmp(&b.pkg))
    });
    seen
}

/// Extract the `applicationId` from a Gradle build file. Handles the Groovy
/// (`applicationId "com.x"`) and Kotlin-DSL (`applicationId = "com.x"`)
/// spellings, ignoring `applicationIdSuffix`.
pub fn parse_application_id(text: &str) -> Option<String> {
    for line in text.lines() {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix("applicationId") else {
            continue;
        };
        // Skip `applicationIdSuffix` and any other `applicationId<x>` token.
        let rest = rest.trim_start();
        if rest.starts_with(|c: char| c.is_alphanumeric()) {
            continue;
        }
        // After the keyword, the first double-quoted run is the id.
        let start = rest.find('"')?;
        let after = &rest[start + 1..];
        let end = after.find('"')?;
        let id = &after[..end];
        if !id.is_empty() {
            return Some(id.to_string());
        }
    }
    None
}

/// Parse `adb shell cmd package resolve-activity --brief <pkg>` output. The
/// brief form ends with a `<pkg>/<activity>` component on its own line; return
/// the activity portion (which may be the `.RelativeName` short form that
/// `am start -n pkg/.RelativeName` accepts).
pub fn parse_resolve_activity(out: &str) -> Option<String> {
    out.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .rev()
        .find_map(|l| l.split_once('/').map(|(_, activity)| activity.to_string()))
        .filter(|a| !a.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avd_list_keeps_names_drops_noise() {
        let out = "\
INFO    | Storing crashdata in: /tmp/foo
Pixel_7_API_34
Tablet_API_33

Nexus_5X.API-30
WARNING: something happened here
";
        assert_eq!(
            parse_avd_list(out),
            vec!["Pixel_7_API_34", "Tablet_API_33", "Nexus_5X.API-30"]
        );
    }

    #[test]
    fn adb_devices_parses_serial_state_model() {
        let out = "\
List of devices attached
emulator-5554          device product:sdk_gphone64_arm64 model:sdk_gphone64_arm64 device:emu64a transport_id:1
0A271FDD40080R         unauthorized usb:338690048X transport_id:2
offlinephone           offline
";
        let devices = parse_adb_devices(out);
        assert_eq!(devices.len(), 3);
        assert_eq!(devices[0].serial, "emulator-5554");
        assert_eq!(devices[0].state, "device");
        assert_eq!(devices[0].model.as_deref(), Some("sdk_gphone64_arm64"));
        assert_eq!(devices[1].state, "unauthorized");
        assert_eq!(devices[1].model, None);
        assert_eq!(devices[2].serial, "offlinephone");
        assert_eq!(devices[2].state, "offline");
    }

    #[test]
    fn sdkmanager_images_split_installed_vs_available() {
        let out = "\
Installed packages:
  Path                                            | Version | Description
  -------                                         | ------- | -------
  build-tools;34.0.0                              | 34.0.0  | Android SDK Build-Tools 34
  system-images;android-34;google_apis;arm64-v8a  | 7       | Google APIs ARM 64 v8a System Image

Available Packages:
  Path                                            | Version | Description
  -------                                         | ------- | -------
  system-images;android-33;google_apis;x86_64     | 12      | Google APIs Intel x86_64
  system-images;android-34;google_apis;arm64-v8a  | 7       | Google APIs ARM 64 v8a System Image
  platforms;android-35                            | 1       | Android SDK Platform 35
";
        let imgs = parse_sdkmanager_images(out);
        // Three distinct system-images rows; the android-34 one is deduped and
        // flagged installed; non-system-image rows are dropped.
        assert_eq!(imgs.len(), 2);
        // Installed sorts first.
        assert_eq!(
            imgs[0].pkg,
            "system-images;android-34;google_apis;arm64-v8a"
        );
        assert!(imgs[0].installed);
        assert_eq!(imgs[1].pkg, "system-images;android-33;google_apis;x86_64");
        assert!(!imgs[1].installed);
    }

    #[test]
    fn sdkmanager_images_handles_separator_lines() {
        // The `------- | -------` separator row contains a pipe but no
        // system-images prefix, so it must be ignored.
        let out = "Installed packages:\n  -------  | ------- | -------\n";
        assert!(parse_sdkmanager_images(out).is_empty());
    }

    #[test]
    fn application_id_groovy_and_kotlin_dsl() {
        let groovy = "android {\n    defaultConfig {\n        applicationId \"com.example.app\"\n        minSdk 24\n    }\n}";
        assert_eq!(
            parse_application_id(groovy).as_deref(),
            Some("com.example.app")
        );
        let kts = "    defaultConfig {\n        applicationId = \"com.acme.thing\"\n    }";
        assert_eq!(parse_application_id(kts).as_deref(), Some("com.acme.thing"));
    }

    #[test]
    fn application_id_ignores_suffix() {
        // `applicationIdSuffix` must not be mistaken for `applicationId`.
        let text = "        applicationIdSuffix \".debug\"\n        applicationId \"com.real.id\"";
        assert_eq!(parse_application_id(text).as_deref(), Some("com.real.id"));
    }

    #[test]
    fn resolve_activity_takes_component_tail() {
        let out = "priority=0 preferredOrder=0 match=0x0\n  com.example.app/.MainActivity";
        assert_eq!(
            parse_resolve_activity(out).as_deref(),
            Some(".MainActivity")
        );
        let fq = "com.example.app/com.example.app.ui.MainActivity\n";
        assert_eq!(
            parse_resolve_activity(fq).as_deref(),
            Some("com.example.app.ui.MainActivity")
        );
    }
}
