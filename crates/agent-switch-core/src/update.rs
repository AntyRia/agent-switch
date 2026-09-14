//! Built-in update channel shared by the CLI and the GUI: check the
//! official GitHub releases for a newer version, download and verify the
//! package for this platform, and install it.
//!
//! Check state is cached in `<config_root>/update-check.json`, so a check
//! on every launch costs one tiny file read; the network is only hit when
//! the cache is older than `CACHE_TTL` (and, after a failure, never more
//! often than once per `FAIL_BACKOFF` unless forced).

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::profile_store::config_root;

/// The public repository the updater reads from.
pub const REPO: &str = "AntyRia/agent-switch";
pub const RELEASES_URL: &str = "https://github.com/AntyRia/agent-switch/releases";
const API_URL: &str = "https://api.github.com/repos/AntyRia/agent-switch/releases/latest";
const USER_AGENT: &str = "agent-switch-updater";
const CACHE_FILE: &str = "update-check.json";
/// How long a successful check stays fresh before re-querying the API
/// (unauthenticated GitHub allows 60 req/h per IP; 6 h is a quiet share).
pub const CACHE_TTL: Duration = Duration::from_secs(6 * 3600);
/// After a failed check, non-forced checks wait this long before retrying
/// (a dead network must not add latency to every launch).
const FAIL_BACKOFF: Duration = Duration::from_secs(3600);
/// Whole-operation cap for package downloads.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);

/// The kind of package to fetch for this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageKind {
    /// The packaged app (macOS DMG / Windows NSIS installer).
    Gui,
    /// The standalone CLI binary (ZIP).
    Cli,
}

/// One downloadable release asset.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
}

/// One GitHub release — only the fields the updater needs; the API returns
/// many more and serde ignores the rest.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Release {
    pub tag_name: String,
    pub html_url: String,
    pub assets: Vec<Asset>,
}

impl Release {
    /// The version without the leading "v" (e.g. "0.2.4").
    pub fn version(&self) -> &str {
        self.tag_name.trim_start_matches('v')
    }
}

/// The outcome of a version check.
pub enum CheckOutcome {
    /// The latest non-prerelease release is known (fresh fetch or cache).
    Known {
        release: Release,
        /// True when served from the local cache (possibly stale after a
        /// failed network fetch).
        cached: bool,
    },
    /// The API was unreachable and no cached answer exists — callers stay
    /// silent instead of surfacing an error.
    Unavailable,
}

/// The running binary's version (all three crates bump in lockstep).
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// True when `latest_tag` (e.g. "v0.2.4") is strictly newer than the
/// running version.
pub fn is_newer(latest_tag: &str) -> bool {
    compare_versions(latest_tag, current_version()) == std::cmp::Ordering::Greater
}

/// Compare two dotted versions; a "v" prefix is tolerated. A pre-release
/// suffix (0.2.4-rc.1) sorts BEFORE the plain release 0.2.4.
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    fn parse(v: &str) -> (u64, u64, u64, Option<&str>) {
        let v = v.trim().trim_start_matches('v');
        let (core, pre) = match v.split_once('-') {
            Some((c, p)) => (c, Some(p)),
            None => (v, None),
        };
        let mut parts = core.split('.').map(|p| p.parse::<u64>().unwrap_or(0));
        (
            parts.next().unwrap_or(0),
            parts.next().unwrap_or(0),
            parts.next().unwrap_or(0),
            pre,
        )
    }
    let pa = parse(a);
    let pb = parse(b);
    pa.0.cmp(&pb.0)
        .then_with(|| pa.1.cmp(&pb.1))
        .then_with(|| pa.2.cmp(&pb.2))
        .then_with(|| match (pa.3, pb.3) {
            (None, None) => std::cmp::Ordering::Equal,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (Some(_), None) => std::cmp::Ordering::Less,
            (Some(x), Some(y)) => x.cmp(y),
        })
}

/// Check for a newer official release.
///
/// `force` bypasses both the freshness cache and the failure backoff (the
/// "check now" button and the `update` command). `timeout` caps the
/// network call. This function never panics: unreachable API without a
/// cache yields `Unavailable`.
pub fn check(force: bool, timeout: Duration) -> CheckOutcome {
    let mut cache = load_cache_at(&cache_path());
    if !force {
        // Fresh cache: answer without any network.
        if let (Some(rel), Some(ts)) = (&cache.release, cache.fetched_at) {
            if elapsed_since(ts).map(|d| d < CACHE_TTL).unwrap_or(false) {
                return CheckOutcome::Known {
                    release: rel.clone(),
                    cached: true,
                };
            }
        }
        // Recent failure: stay on the (stale) cache or stay silent.
        if let Some(ts) = cache.failed_at {
            if elapsed_since(ts).map(|d| d < FAIL_BACKOFF).unwrap_or(false) {
                return match cache.release.take() {
                    Some(r) => CheckOutcome::Known {
                        release: r,
                        cached: true,
                    },
                    None => CheckOutcome::Unavailable,
                };
            }
        }
    }
    match fetch_latest(timeout) {
        Ok(release) => {
            cache.release = Some(release.clone());
            cache.fetched_at = Some(now_unix());
            cache.failed_at = None;
            store_cache_at(&cache_path(), &cache);
            CheckOutcome::Known {
                release,
                cached: false,
            }
        }
        Err(_) => {
            cache.failed_at = Some(now_unix());
            store_cache_at(&cache_path(), &cache);
            match cache.release {
                Some(r) => CheckOutcome::Known {
                    release: r,
                    cached: true,
                },
                None => CheckOutcome::Unavailable,
            }
        }
    }
}

/// The package asset for THIS machine, if the release ships one.
pub fn asset_for(release: &Release, kind: PackageKind) -> Option<Asset> {
    let v = release.version();
    let name = match (kind, std::env::consts::OS) {
        (PackageKind::Gui, "macos") => match std::env::consts::ARCH {
            "aarch64" => format!("Agent.Switch_{v}_aarch64.dmg"),
            _ => format!("Agent.Switch_{v}_x64.dmg"),
        },
        (PackageKind::Gui, "windows") => format!("Agent.Switch_{v}_x64-setup.exe"),
        (PackageKind::Cli, "macos") => match std::env::consts::ARCH {
            "aarch64" => format!("agent-switch-{v}-aarch64-apple-darwin.zip"),
            _ => format!("agent-switch-{v}-x86_64-apple-darwin.zip"),
        },
        (PackageKind::Cli, "windows") => format!("agent-switch-{v}-x86_64-pc-windows-msvc.zip"),
        (PackageKind::Cli, "linux") => match std::env::consts::ARCH {
            "aarch64" => format!("agent-switch-{v}-aarch64-unknown-linux-gnu.zip"),
            _ => format!("agent-switch-{v}-x86_64-unknown-linux-gnu.zip"),
        },
        // No packaged build for this combination (e.g. the GUI on Linux).
        _ => return None,
    };
    release
        .assets
        .iter()
        .find(|a| a.name == name)
        .cloned()
}

/// Stream `url` to `dest`, calling `progress(done_bytes, total_bytes)`
/// (total 0 when the server sent no Content-Length). Returns the bytes
/// written.
pub fn download(url: &str, dest: &Path, mut progress: impl FnMut(u64, u64)) -> Result<u64> {
    let mut file = fs::File::create(dest)?;
    let resp = ureq::get(url)
        .timeout(DOWNLOAD_TIMEOUT)
        .set("User-Agent", USER_AGENT)
        .call()
        .map_err(|e| Error::Other(format!("download failed: {e}")))?;
    let total = resp
        .header("content-length")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    let mut reader = resp.into_reader();
    let mut buf = [0u8; 64 * 1024];
    let mut done: u64 = 0;
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| Error::Other(format!("download failed: {e}")))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
        done += n as u64;
        progress(done, total);
    }
    Ok(done)
}

/// The SHA-256 of a file, lowercase hex.
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex_encode(&h.finalize()))
}

/// Parse `<64-hex>  <file>` lines (any whitespace run between the two).
pub fn parse_sums(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let (digest, name) = line.split_once(char::is_whitespace)?;
            let digest = digest.trim();
            if digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit()) {
                Some((digest.to_ascii_lowercase(), name.trim().to_string()))
            } else {
                None
            }
        })
        .collect()
}

/// Verify `file` against the release's SHA256SUMS.txt when it lists
/// `asset_name`: Ok(Some(digest)) = verified, Ok(None) = the release ships
/// no checksum for this asset (verification not possible), Err = mismatch
/// or a broken checksum file.
pub fn verify_against_sums(
    release: &Release,
    asset_name: &str,
    file: &Path,
) -> Result<Option<String>> {
    let Some(sums_asset) = release
        .assets
        .iter()
        .find(|a| a.name == "SHA256SUMS.txt")
    else {
        return Ok(None);
    };
    let tmp = file.with_extension("sums.tmp");
    download(&sums_asset.browser_download_url, &tmp, |_, _| {})?;
    let text = fs::read_to_string(&tmp)?;
    let _ = fs::remove_file(&tmp);
    // parse_sums yields (digest, name) pairs.
    let Some(expected) = parse_sums(&text)
        .into_iter()
        .find(|(_, name)| name == asset_name)
        .map(|(digest, _)| digest)
    else {
        return Ok(None);
    };
    let actual = sha256_file(file)?;
    if !actual.eq_ignore_ascii_case(&expected) {
        return Err(Error::Other(format!(
            "checksum mismatch for {asset_name}: expected {expected}, got {actual}"
        )));
    }
    Ok(Some(actual))
}

/// Extract the CLI binary from a release ZIP to `dest` (the binary may sit
/// at the zip root or in a top-level folder).
pub fn extract_cli_binary(zip_path: &Path, dest: &Path) -> Result<()> {
    let file = fs::File::open(zip_path)?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| Error::Other(format!("zip open failed: {e}")))?;
    let wanted = if cfg!(windows) { "agent-switch.exe" } else { "agent-switch" };
    let mut found = false;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| Error::Other(format!("zip read failed: {e}")))?;
        let name = entry.name();
        let stem = name.rsplit('/').next().unwrap_or(name);
        if entry.is_file() && stem == wanted {
            let mut out = fs::File::create(dest)?;
            std::io::copy(&mut entry, &mut out)?;
            found = true;
            break;
        }
    }
    if !found {
        return Err(Error::Other(format!(
            "binary '{wanted}' not found in {}",
            zip_path.display()
        )));
    }
    Ok(())
}

/// Atomically replace `current` (the running binary's path) with `new_bin`.
/// A running Windows exe cannot be deleted in place, so it is renamed
/// aside first (the process keeps its open handle) and the leftover is
/// removed by the next launch via [`remove_stale_old`].
pub fn replace_binary(current: &Path, new_bin: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        let old = with_suffix(current, "old");
        if current.exists() {
            fs::rename(current, &old)?;
        }
        fs::rename(new_bin, current)?;
        // May fail while this process still runs; retried at next start.
        let _ = fs::remove_file(&old);
        return Ok(());
    }
    #[cfg(not(windows))]
    {
        fs::rename(new_bin, current)?;
        Ok(())
    }
}

/// Remove a leftover "<binary>.old" from a previous Windows self-update.
/// Best-effort: called at every CLI start.
pub fn remove_stale_old(current: &Path) {
    let _ = fs::remove_file(with_suffix(current, "old"));
}

/// The .app bundle of the running GUI (None outside a packaged bundle,
/// e.g. a dev build).
#[cfg(target_os = "macos")]
pub fn macos_bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    // current_exe = <bundle>/Contents/MacOS/<binary>
    let bundle = exe.parent()?.parent()?;
    matches!(
        bundle.extension().and_then(|e| e.to_str()),
        Some("app")
    )
    .then(|| bundle.to_path_buf())
}

/// Write the detached macOS self-update script and spawn it (fire and
/// forget — it survives the app's exit). Returns the script path.
///
/// The script waits for `app_pid` to exit, refuses to run while another
/// instance of the app is alive, then mounts the DMG, replaces the bundle
/// (rm + ditto), strips the quarantine xattr and relaunches the app.
#[cfg(target_os = "macos")]
pub fn spawn_macos_updater(app_pid: u32, bundle: &Path, dmg: &Path) -> Result<PathBuf> {
    let script = std::env::temp_dir().join(format!(
        "agent-switch-update-{}.sh",
        uuid::Uuid::new_v4()
    ));
    let app_name = bundle
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("Agent Switch.app");
    // format! template: every literal { } of the shell script is doubled.
    let text = format!(
        r###"#!/bin/sh
# Agent Switch self-updater — runs detached after the app exits.
APP="{app}"
DMG="{dmg}"
APP_NAME="{app_name}"
SCRIPT="{script}"
MOUNT=""
cleanup() {{
  [ -n "$MOUNT" ] && hdiutil detach "$MOUNT" -quiet 2>/dev/null
  rm -f "$DMG" "$SCRIPT"
}}
# 1. Wait for the app process to exit (up to 2 minutes).
i=0
while [ "$i" -lt 240 ]; do
  if ! kill -0 {pid} 2>/dev/null; then break; fi
  i=$((i+1))
  sleep 0.5
done
kill -0 {pid} 2>/dev/null && exit 1
# 2. Refuse while another instance of this app is still running.
pgrep -f "$APP_NAME/Contents/MacOS" >/dev/null 2>&1 && exit 1
# 3. Mount the DMG, replace the bundle, relaunch.
MOUNT=$(hdiutil attach -nobrowse -noautoopen "$DMG" | awk -F'\t' '/\/Volumes\// {{print $NF; exit}}')
[ -n "$MOUNT" ] || exit 1
[ -d "$MOUNT/$APP_NAME" ] || {{ hdiutil detach "$MOUNT" -quiet 2>/dev/null; exit 1; }}
rm -rf "$APP" || exit 1
ditto "$MOUNT/$APP_NAME" "$APP" || {{ hdiutil detach "$MOUNT" -quiet 2>/dev/null; exit 1; }}
xattr -dr com.apple.quarantine "$APP" 2>/dev/null
cleanup
open "$APP"
"###,
        app = bundle.display(),
        dmg = dmg.display(),
        app_name = app_name,
        script = script.display(),
        pid = app_pid,
    );
    fs::write(&script, text)?;
    let _ = std::process::Command::new("/bin/sh")
        .arg(&script)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    Ok(script)
}

/// Spawn a detached `cmd` that waits 3 s (for the app to exit) and then
/// launches the NSIS installer, which replaces the installed files.
#[cfg(windows)]
pub fn spawn_windows_installer(exe: &Path) -> Result<()> {
    let line = format!(
        "timeout /t 3 /nobreak >nul & start \"\" \"{}\"",
        exe.display()
    );
    let _ = std::process::Command::new("cmd.exe")
        .args(["/C", &line])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    Ok(())
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("agent-switch");
    path.with_file_name(format!("{name}.{suffix}"))
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    bytes
        .iter()
        .flat_map(|b| [HEX[(*b >> 4) as usize], HEX[(b & 0xf) as usize]])
        .map(char::from)
        .collect()
}

fn fetch_latest(timeout: Duration) -> Result<Release> {
    let text = ureq::get(API_URL)
        .timeout(timeout)
        .set("User-Agent", USER_AGENT)
        .call()
        .map_err(|e| Error::Other(format!("release check failed: {e}")))?
        .into_string()
        .map_err(|e| Error::Other(format!("release check failed: {e}")))?;
    serde_json::from_str(&text).map_err(|e| Error::Other(format!("release check failed: {e}")))
}

fn cache_path() -> PathBuf {
    config_root().join(CACHE_FILE)
}

fn load_cache_at(path: &Path) -> CacheFile {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn store_cache_at(path: &Path, c: &CacheFile) {
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(path, serde_json::to_string(c).unwrap_or_default());
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn elapsed_since(ts: u64) -> Option<Duration> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH + Duration::from_secs(ts))
        .ok()
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct CacheFile {
    /// Unix seconds of the last successful fetch (None = never).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fetched_at: Option<u64>,
    /// Unix seconds of the last failed fetch (retry backoff).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    failed_at: Option<u64>,
    /// The release from the last successful fetch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    release: Option<Release>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, assets: &[&str]) -> Release {
        Release {
            tag_name: tag.to_string(),
            html_url: format!("{RELEASES_URL}/{tag}"),
            assets: assets
                .iter()
                .map(|a| Asset {
                    name: a.to_string(),
                    browser_download_url: format!("https://example.invalid/{a}"),
                })
                .collect(),
        }
    }

    const MACOS_V023: &[&str] = &[
        "agent-switch-0.2.3-aarch64-apple-darwin.zip",
        "agent-switch-0.2.3-x86_64-apple-darwin.zip",
        "Agent.Switch_0.2.3_aarch64.dmg",
        "Agent.Switch_0.2.3_x64.dmg",
        "SHA256SUMS.txt",
    ];
    const WIN_V023: &[&str] = &[
        "agent-switch-0.2.3-x86_64-pc-windows-msvc.zip",
        "Agent.Switch_0.2.3_x64-setup.exe",
        "Agent.Switch.Setup.0.2.3.x64.msi",
    ];

    #[test]
    fn version_comparison() {
        use std::cmp::Ordering::*;
        assert_eq!(compare_versions("v0.2.4", "0.2.3"), Greater);
        assert_eq!(compare_versions("0.2.3", "0.2.3"), Equal);
        assert_eq!(compare_versions("0.2.3", "v0.2.4"), Less);
        assert_eq!(compare_versions("0.10.0", "0.9.9"), Greater);
        assert_eq!(compare_versions("1.0.0", "0.99.99"), Greater);
        // Pre-release sorts before its release.
        assert_eq!(compare_versions("0.2.4-rc.1", "0.2.4"), Less);
        assert_eq!(compare_versions("0.2.4", "0.2.4-rc.1"), Greater);
        assert_eq!(compare_versions("0.2.4-rc.1", "0.2.4-rc.2"), Less);
        // Malformed parts fall back to 0, never panic.
        assert_eq!(compare_versions("0.x.3", "0.0.3"), Equal);
        assert_eq!(compare_versions("", ""), Equal);
    }

    #[test]
    fn is_newer_tracks_running_version() {
        let cur = current_version();
        assert!(!is_newer(&format!("v{cur}")));
        // Bumping the last component always yields "newer".
        let parts: Vec<&str> = cur.split('.').collect();
        let last: u64 = parts.last().unwrap().parse().unwrap_or(0);
        let newer = format!("v{}.{}.{}", parts[0], parts[1], last + 1);
        assert!(is_newer(&newer));
    }

    #[test]
    fn asset_selection_matches_platform() {
        let r = release("v0.2.3", MACOS_V023);
        let name = |a: &Asset| a.name.clone();
        match std::env::consts::OS {
            "macos" => {
                let gui = asset_for(&r, PackageKind::Gui).expect("macos gui asset");
                assert!(gui.name.ends_with(".dmg"));
                let cli = asset_for(&r, PackageKind::Cli).expect("macos cli asset");
                assert!(cli.name.ends_with(".zip"));
                assert!(cli.name.contains("-apple-darwin"));
            }
            "windows" => {
                // The macOS release has no Windows assets.
                assert!(asset_for(&r, PackageKind::Gui).is_none());
                assert!(asset_for(&r, PackageKind::Cli).is_none());
            }
            _ => {}
        }
        let w = release("v0.2.3", WIN_V023);
        if std::env::consts::OS == "windows" {
            let gui = asset_for(&w, PackageKind::Gui).expect("windows gui asset");
            assert!(gui.name.ends_with("_x64-setup.exe"));
            let cli = asset_for(&w, PackageKind::Cli).expect("windows cli asset");
            assert!(name(&cli)
                == "agent-switch-0.2.3-x86_64-pc-windows-msvc.zip");
        }
        // A release without this platform's package yields None.
        assert!(asset_for(&w, PackageKind::Gui)
            .filter(|a| a.name.ends_with(".dmg"))
            .is_none());
    }

    #[test]
    fn sums_parsing() {
        let text = "6912fd72149ba451acb25aa4ef231b2389720e8f1f69514ef50ca04a6ba9c93a  Agent.Switch_0.2.3_aarch64.dmg\n\
                    1dd0e13e91f0a6307ee4a5e96304f49435333ecb27585bad3f2e80df741cc5b5  agent-switch-0.2.3-aarch64-apple-darwin.zip\n\
                    not-a-checksum  some-file\n\
                    \n";
        let sums = parse_sums(text);
        assert_eq!(sums.len(), 2);
        assert_eq!(sums[0].1, "Agent.Switch_0.2.3_aarch64.dmg");
        assert_eq!(
            sums[1].0,
            "1dd0e13e91f0a6307ee4a5e96304f49435333ecb27585bad3f2e80df741cc5b5"
        );
    }

    #[test]
    fn sha256_of_known_bytes() {
        let dir = std::env::temp_dir().join(format!(
            "as-update-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.bin");
        fs::write(&f, b"abc").unwrap();
        // SHA-256("abc") is a well-known vector.
        assert_eq!(
            sha256_file(&f).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn cache_roundtrip() {
        let dir = std::env::temp_dir().join(format!(
            "as-update-cache-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let path = dir.join(CACHE_FILE);
        let c = CacheFile {
            fetched_at: Some(now_unix()),
            failed_at: None,
            release: Some(release("v0.2.4", &["SHA256SUMS.txt"])),
        };
        store_cache_at(&path, &c);
        let back = load_cache_at(&path);
        assert_eq!(back.release.as_ref().unwrap().version(), "0.2.4");
        assert!(back.fetched_at.is_some());
        assert!(back.failed_at.is_none());
        // A corrupt cache file reads as empty, never errors.
        fs::write(&path, "not json").unwrap();
        assert!(load_cache_at(&path).release.is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}
