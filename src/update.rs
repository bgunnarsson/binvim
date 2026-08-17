//! "Is a newer binvim out?" check.
//!
//! One GET against the crates.io metadata endpoint, cached in
//! `<cache_dir>/update-check.json` so the network call happens at most once a
//! day no matter how often the editor is launched. crates.io is the source of
//! truth because it's the first target `scripts/release.sh` publishes to —
//! Homebrew and binvim.dev follow it, so a version visible there is a version
//! every install path can reach.
//!
//! Everything here runs on a background thread ([`check`] blocks on `curl`),
//! and every failure is silent: the user didn't ask for this check, so a
//! flaky network or an offline machine must not produce an error notification.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;

use crate::paths;

/// crates.io's per-crate metadata endpoint. Returns the full version list plus
/// the `max_stable_version` / `newest_version` summary fields we read.
const CRATES_IO_URL: &str = "https://crates.io/api/v1/crates/binvim";

/// How long a fetched result stays fresh. A day is the granularity of a
/// release cadence — checking more often costs a request per launch and tells
/// the user nothing new.
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// The version this binary was built from.
pub fn current() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// `Some(latest)` when the newest published release is ahead of ours, `None`
/// when we're current. Blocks on a subprocess — call from a background thread.
///
/// The cache is consulted first and, when fresh, answers without touching the
/// network at all. That's what makes the start-page banner appear immediately
/// on every launch after the first: the "check" is a file read.
pub fn check() -> Result<Option<String>, String> {
    let latest = match cached_latest() {
        Some(v) => v,
        None => {
            let body = crate::package::http_get(CRATES_IO_URL)?;
            let latest = parse_crates_io_latest(&body)?;
            write_cache(&latest);
            latest
        }
    };
    Ok(is_newer(&latest, current()).then_some(latest))
}

fn cache_path() -> Option<PathBuf> {
    paths::cache_dir().map(|d| d.join("update-check.json"))
}

#[derive(Deserialize)]
struct CachedCheck {
    /// Unix seconds at which `latest` was fetched.
    checked_at: u64,
    latest: String,
}

/// The cached version, if the file exists and is younger than
/// [`CHECK_INTERVAL`]. A clock that has moved backwards (`checked_at` in the
/// future) reads as stale rather than as fresh-forever.
fn cached_latest() -> Option<String> {
    let text = std::fs::read_to_string(cache_path()?).ok()?;
    let cached: CachedCheck = serde_json::from_str(&text).ok()?;
    let age = now_secs().checked_sub(cached.checked_at)?;
    (age < CHECK_INTERVAL.as_secs()).then_some(cached.latest)
}

fn write_cache(latest: &str) {
    let Some(path) = cache_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let body = serde_json::json!({ "checked_at": now_secs(), "latest": latest });
    let _ = std::fs::write(path, body.to_string());
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Pull the newest release out of a crates.io `GET /api/v1/crates/binvim`
/// body. Prefers `max_stable_version` so a published prerelease never nags a
/// user on a stable build; falls back to `newest_version` for the (impossible
/// in practice, but cheap to cover) case of a crate with no stable release.
fn parse_crates_io_latest(body: &str) -> Result<String, String> {
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("crates.io response not JSON: {e}"))?;
    let krate = &v["crate"];
    krate["max_stable_version"]
        .as_str()
        .or_else(|| krate["newest_version"].as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "crates.io response had no version field".to_string())
}

/// True when `latest` is a strictly newer release than `current`. Unparseable
/// input on either side answers `false` — a version string we don't understand
/// is not grounds for telling the user to upgrade.
pub fn is_newer(latest: &str, current: &str) -> bool {
    let (Some(l), Some(c)) = (parse_version(latest), parse_version(current)) else {
        return false;
    };
    l > c
}

/// Semver core + prerelease, ordered so that a release beats the prereleases
/// that led to it (0.6.0 > 0.6.0-rc.1). Build metadata (`+…`) is dropped —
/// semver says it doesn't participate in precedence.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct Version {
    major: u64,
    minor: u64,
    patch: u64,
    pre: Prerelease,
}

/// `Release` sorts above `Pre` because `derive(Ord)` on an enum orders by
/// variant declaration, which is exactly the semver rule.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum Prerelease {
    Pre(Vec<PreIdent>),
    Release,
}

/// Numeric prerelease identifiers compare numerically and sort below
/// alphanumeric ones (`rc.2` > `rc.1`, but `beta` > `1`).
#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum PreIdent {
    Num(u64),
    Text(String),
}

fn parse_version(s: &str) -> Option<Version> {
    let s = s.trim().trim_start_matches('v');
    let s = s.split('+').next()?;
    let (core, pre) = match s.split_once('-') {
        Some((core, pre)) => (core, Prerelease::Pre(parse_prerelease(pre))),
        None => (s, Prerelease::Release),
    };
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(Version {
        major,
        minor,
        patch,
        pre,
    })
}

fn parse_prerelease(s: &str) -> Vec<PreIdent> {
    s.split('.')
        .map(|id| match id.parse::<u64>() {
            Ok(n) => PreIdent::Num(n),
            Err(_) => PreIdent::Text(id.to_string()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_patch_minor_major() {
        assert!(is_newer("0.5.17", "0.5.16"));
        assert!(is_newer("0.6.0", "0.5.16"));
        assert!(is_newer("1.0.0", "0.9.9"));
    }

    #[test]
    fn same_or_older_is_not_newer() {
        assert!(!is_newer("0.5.16", "0.5.16"));
        assert!(!is_newer("0.5.15", "0.5.16"));
        assert!(!is_newer("0.4.99", "0.5.0"));
    }

    #[test]
    fn numeric_segments_compare_numerically() {
        // Lexical comparison would put "0.5.9" ahead of "0.5.10".
        assert!(is_newer("0.5.10", "0.5.9"));
        assert!(!is_newer("0.5.9", "0.5.10"));
    }

    #[test]
    fn release_beats_its_own_prerelease() {
        assert!(is_newer("0.6.0", "0.6.0-rc.1"));
        assert!(!is_newer("0.6.0-rc.1", "0.6.0"));
        assert!(is_newer("0.6.0-rc.2", "0.6.0-rc.1"));
        // A stable release older than the prerelease we're running is not news.
        assert!(!is_newer("0.5.16", "0.6.0-rc.1"));
    }

    #[test]
    fn build_metadata_is_ignored() {
        assert!(!is_newer("0.5.16+build.7", "0.5.16"));
        assert!(is_newer("0.5.17+build.7", "0.5.16"));
    }

    #[test]
    fn unparseable_versions_never_nag() {
        assert!(!is_newer("nightly", "0.5.16"));
        assert!(!is_newer("0.5.17", "not-a-version"));
        assert!(!is_newer("0.5.16.1", "0.5.16"));
    }

    #[test]
    fn crates_io_prefers_max_stable() {
        let body = r#"{"crate":{"id":"binvim","max_stable_version":"0.5.17",
            "newest_version":"0.6.0-rc.1","max_version":"0.6.0-rc.1"}}"#;
        assert_eq!(parse_crates_io_latest(body).unwrap(), "0.5.17");
    }

    #[test]
    fn crates_io_falls_back_to_newest() {
        let body = r#"{"crate":{"id":"binvim","newest_version":"0.1.0-alpha.1"}}"#;
        assert_eq!(parse_crates_io_latest(body).unwrap(), "0.1.0-alpha.1");
    }

    #[test]
    fn crates_io_garbage_errors() {
        assert!(parse_crates_io_latest("<html>502</html>").is_err());
        assert!(parse_crates_io_latest(r#"{"crate":{}}"#).is_err());
    }

    #[test]
    fn our_own_version_parses() {
        // Guards the pin in Cargo.toml against a shape this module can't read
        // — an unparseable current version silently disables the whole check.
        assert!(parse_version(current()).is_some());
    }
}
