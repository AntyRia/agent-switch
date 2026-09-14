//! Resolving the Codex/Claude CLI binaries for shell-less processes.
//!
//! A GUI app launched from the Dock, Finder or Start menu inherits the
//! windowing system's minimal PATH (macOS: `/usr/bin:/bin:/usr/sbin:/sbin`)
//! and never sources the user's shell profile, so `which` alone misses the
//! most common install styles: Homebrew, npm/bun/volta globals and node
//! version managers (nvm, fnm, asdf, mise). Resolution therefore falls
//! through three layers:
//!
//! 1. the process's own PATH (`which`) — covers terminal-launched runs;
//! 2. well-known install locations (see `candidate_dirs`) — covers the
//!    standard installers without running anything;
//! 3. Unix only: the user's login+interactive shell
//!    (`$SHELL -l -i -c 'printf %s "$PATH"'`, time-capped) — covers any
//!    custom PATH the user configured in their profile.
//!
//! Windows skips layer 3: Explorer-launched processes inherit the
//! registry user+machine PATH, which is exactly where npm, nvm-windows,
//! fnm and volta register their global bin dirs, so layer 2 is the only
//! gap that can remain there.
//!
//! Results are cached per process: positive hits are re-validated with a
//! cheap stat on every lookup (a moved binary is re-resolved), negative
//! hits expire after `NEGATIVE_TTL` so a freshly installed CLI is picked
//! up; `clear_binary_cache` (called on forced status refresh) drops both.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(unix)]
use std::process::Stdio;
#[cfg(unix)]
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How long a "not found" answer stays valid. Kept short: the expected flow
/// is install → click "re-detect", and the user must not wait a minute for
/// the answer to flip.
const NEGATIVE_TTL: Duration = Duration::from_secs(30);
/// Hard cap for the login-shell probe. A slow or broken profile must not
/// stall the app; on timeout the probe is abandoned and the binary simply
/// reports as not found.
#[cfg(unix)]
const SHELL_PROBE_TIMEOUT: Duration = Duration::from_secs(4);

// Cache entry: (when, resolved value). Shared shape for both caches.
type When<T> = (Instant, Option<T>);
// OnceLock because `HashMap::new()` is not const (RandomState).
static CACHE: OnceLock<Mutex<HashMap<String, When<PathBuf>>>> = OnceLock::new();
static LOGIN_SHELL_DIRS: OnceLock<Mutex<Option<When<Vec<PathBuf>>>>> = OnceLock::new();

/// Resolve a CLI binary the way the user's shell would find it.
///
/// `None` means "not installed anywhere we can see" — the caller maps it to
/// its engine-specific not-found error. Safe to call from any thread.
pub fn resolve_binary(name: &str) -> Option<PathBuf> {
    {
        let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        let cache = cache.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((at, hit)) = cache.get(name) {
            match hit {
                // Positive hits are re-validated: if the binary moved, the
                // lookup falls through and re-resolves.
                Some(p) if p.exists() => return Some(p.clone()),
                None if at.elapsed() < NEGATIVE_TTL => return None,
                _ => {}
            }
        }
    }
    let found = which::which(name)
        .ok()
        .or_else(|| known_location_for(name))
        .or_else(|| login_shell_dirs().and_then(|dirs| dirs.iter().find_map(|d| find_in_dir(d, name))));
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(mut cache) = cache.lock() {
        cache.insert(name.to_string(), (Instant::now(), found.clone()));
    }
    found
}

/// Drop all cached resolutions (positive and negative). Called on a forced
/// status refresh so a CLI installed while the app is running is found
/// immediately, without waiting for the negative TTL.
pub fn clear_binary_cache() {
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(mut cache) = cache.lock() {
        cache.clear();
    }
    let dirs = LOGIN_SHELL_DIRS.get_or_init(|| Mutex::new(None));
    if let Ok(mut dirs) = dirs.lock() {
        *dirs = None;
    }
}

/// Prepend the binary's own directory to a child's inherited PATH. Both
/// CLIs are `#!/usr/bin/env node` scripts, so spawning one from a GUI
/// process (shell-less PATH) fails at the shebang lookup itself without
/// this — the runtime (node) lives next to the CLI.
pub fn prepend_bin_dir_to_path(cmd: &mut Command, bin: &Path) {
    let Some(dir) = bin.parent().filter(|d| d.is_absolute()) else {
        return;
    };
    #[cfg(windows)]
    const SEP: &str = ";";
    #[cfg(not(windows))]
    const SEP: &str = ":";
    let mut joined = std::ffi::OsString::from(dir.as_os_str());
    joined.push(SEP);
    joined.push(std::env::var_os("PATH").unwrap_or_default());
    cmd.env("PATH", joined);
}

/// Well-known bin dirs, in preference order (no de-duplication needed: the
/// first executable hit wins, and stat'ing a few dozen dirs is trivial).
fn candidate_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(home) = dirs::home_dir() {
        // Per-user package manager locations (all npm-ecosystem CLIs).
        dirs.push(home.join(".local/bin"));
        dirs.push(home.join(".npm-global/bin"));
        dirs.push(home.join(".local/share/pnpm"));
        dirs.push(home.join(".bun/bin"));
        dirs.push(home.join(".volta/bin"));
        dirs.push(home.join(".yarn/bin"));
        // Node version managers: prefer the user's "default" alias version
        // when present, otherwise the newest install.
        let nvm_root = home.join(".nvm");
        let nvm_alias = nvm_root.join("alias/default");
        dirs.extend(node_version_bins(
            &nvm_root.join("versions/node"),
            "bin",
            Some(&nvm_alias),
        ));
        for root in fnm_roots(&home) {
            let alias = root.join("aliases/default");
            dirs.extend(node_version_bins(
                &root.join("node-versions"),
                "installation/bin",
                Some(&alias),
            ));
        }
        let asdf = home.join(".asdf");
        dirs.extend(node_version_bins(&asdf.join("installs/nodejs"), "bin", None));
        let mise = home.join(".local/share/mise");
        dirs.extend(node_version_bins(&mise.join("installs/nodejs"), "bin", None));
    }
    #[cfg(target_os = "macos")]
    {
        // Homebrew (Apple Silicon first, Intel second), then the historic
        // x86 prefix — the two dirs a GUI app's launchd PATH never has.
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
        dirs.push(PathBuf::from("/usr/local/bin"));
    }
    #[cfg(target_os = "linux")]
    dirs.push(PathBuf::from("/usr/local/bin"));
    #[cfg(windows)]
    {
        // npm global (%APPDATA%\npm, registered in the user PATH but listed
        // here too), the nvm-windows node symlink and scoop shims.
        if let Some(appdata) = std::env::var_os("APPDATA") {
            dirs.push(PathBuf::from(appdata).join("npm"));
        }
        let nvm_symlink = std::env::var_os("NVM_SYMLINK")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Program Files\nodejs"));
        dirs.push(nvm_symlink);
        if let Some(home) = dirs::home_dir() {
            dirs.push(home.join("scoop/shims"));
        }
    }
    dirs
}

fn known_location_for(name: &str) -> Option<PathBuf> {
    candidate_dirs()
        .into_iter()
        .find_map(|dir| find_in_dir(&dir, name))
}

/// The first usable `<dir>/<name>`: on Unix the bare file (executable bit
/// required); on Windows the PATHEXT-suffixed variants first, because npm's
/// extensionless shim is a POSIX script PowerShell cannot run.
fn find_in_dir(dir: &Path, name: &str) -> Option<PathBuf> {
    let bare = dir.join(name);
    #[cfg(not(windows))]
    {
        if is_executable(&bare) {
            return Some(bare);
        }
        None
    }
    #[cfg(windows)]
    {
        for ext in ["exe", "cmd", "bat", "ps1"] {
            let p = dir.join(format!("{name}.{ext}"));
            if is_executable(&p) {
                return Some(p);
            }
        }
        if is_executable(&bare) {
            return Some(bare);
        }
        None
    }
}

fn is_executable(p: &Path) -> bool {
    if !p.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        p.metadata()
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// fnm's data dir moved over the years; check the historical locations.
fn fnm_roots(home: &Path) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        let base = PathBuf::from(xdg);
        roots.push(if base.is_absolute() { base } else { home.join(base) }.join("fnm"));
    }
    roots.push(home.join(".local/share/fnm"));
    roots.push(home.join(".fnm"));
    #[cfg(windows)]
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        roots.push(PathBuf::from(local).join("fnm"));
    }
    roots
}

/// Bin dirs under a version manager tree (`<versions_dir>/<version>/…`),
/// ordered: the version named by the `default` alias file first (exact
/// match, then prefix — nvm aliases are "20", "v20.11.0" or "lts/…" and
/// only the first two are resolvable here), the rest newest-first.
fn node_version_bins(versions_dir: &Path, bin_rel: &str, alias_file: Option<&Path>) -> Vec<PathBuf> {
    let entries = match std::fs::read_dir(versions_dir) {
        Ok(e) => e.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect::<Vec<_>>(),
        Err(_) => return Vec::new(),
    };
    if entries.is_empty() {
        return Vec::new();
    }
    let alias = alias_file
        .and_then(|f| std::fs::read_to_string(f).ok())
        .map(|raw| raw.trim().to_string())
        .filter(|a| !a.is_empty());
    let name_of = |p: &Path| {
        p.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string()
    };
    let mut versions = entries;
    versions.sort_by(|a, b| {
        let (na, nb) = (name_of(a), name_of(b));
        let ra = alias_rank(&na, alias.as_deref());
        let rb = alias_rank(&nb, alias.as_deref());
        ra.cmp(&rb).then_with(|| version_key(&nb).cmp(&version_key(&na)))
    });
    versions
        .into_iter()
        .map(|v| v.join(bin_rel))
        .filter(|p| p.is_dir())
        .collect()
}

fn alias_rank(version: &str, alias: Option<&str>) -> u8 {
    let v = version.strip_prefix('v').unwrap_or(version);
    match alias {
        Some(a) => {
            let a = a.strip_prefix('v').unwrap_or(a);
            if v == a {
                0
            } else if v.starts_with(a) {
                1
            } else {
                2
            }
        }
        None => 2,
    }
}

/// Numeric components of a version dir name ("v20.11.0" → [20, 11, 0]) so
/// "20.5.0" sorts above "18.19.0" — lexicographic order would not.
fn version_key(name: &str) -> Vec<u64> {
    name.trim_start_matches('v')
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .take(4)
        .map(|s| s.parse::<u64>().unwrap_or(0))
        .collect()
}

/// The login shell's PATH, probed at most once per process (retried after
/// `NEGATIVE_TTL` on failure). Unix only — see the module docs for why
/// Windows skips this layer.
fn login_shell_dirs() -> Option<Vec<PathBuf>> {
    let store = LOGIN_SHELL_DIRS.get_or_init(|| Mutex::new(None));
    {
        let guard = store.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((at, dirs)) = &*guard {
            if dirs.is_some() || at.elapsed() < NEGATIVE_TTL {
                return dirs.clone();
            }
        }
    }
    #[cfg(unix)]
    let dirs = probe_login_shell_path();
    #[cfg(not(unix))]
    let dirs = None;
    if let Ok(mut guard) = store.lock() {
        *guard = Some((Instant::now(), dirs.clone()));
    }
    dirs
}

#[cfg(unix)]
fn probe_login_shell_path() -> Option<Vec<PathBuf>> {
    let shell = std::env::var("SHELL")
        .ok()?
        .trim()
        .to_string();
    if shell.is_empty() {
        return None;
    }
    let shell_bin = if shell.starts_with('/') {
        let p = PathBuf::from(&shell);
        p.is_file().then_some(p)
    } else {
        which::which(&shell).ok()
    };
    let shell_bin = shell_bin?;
    // -l sources the login profile (.zprofile/.bash_profile), -i the
    // interactive rc files (.zshrc/.bashrc) — PATH usually lands in one or
    // the other. stderr is discarded (rc files may be chatty) and only a
    // clean absolute path list on stdout is accepted, so profile output
    // can at worst cost us the probe, never a wrong answer.
    let out = run_capped(&shell_bin, &["-l", "-i", "-c", "printf '%s' \"$PATH\""])?;
    parse_path_var(&String::from_utf8_lossy(&out))
}

/// Run a command with a hard wall-clock cap: stdout is read on a worker
/// thread; on timeout the child is killed and `None` returned.
#[cfg(unix)]
fn run_capped(program: &Path, args: &[&str]) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut child = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let mut stdout = stdout;
        let _ = stdout.read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    match rx.recv_timeout(SHELL_PROBE_TIMEOUT) {
        Ok(buf) => {
            let _ = child.wait();
            Some(buf)
        }
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            None
        }
    }
}

/// Split a `$PATH` dump and keep it only when it looks exactly like a path
/// variable: non-empty, and every entry absolute. Banners and rc-file
/// chatter therefore fail the check instead of leaking in as candidates.
#[cfg(unix)]
fn parse_path_var(out: &str) -> Option<Vec<PathBuf>> {
    let line = out.lines().rev().map(str::trim).find(|l| !l.is_empty())?;
    #[cfg(windows)]
    let sep = ';';
    #[cfg(not(windows))]
    let sep = ':';
    let parts: Vec<PathBuf> = line.split(sep).map(PathBuf::from).collect();
    if parts.is_empty() || !parts.iter().all(|p| p.is_absolute()) {
        return None;
    }
    Some(parts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[cfg(unix)]
    fn make_executable(p: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let mut m = fs::metadata(p).unwrap().permissions();
        m.set_mode(0o755);
        fs::set_permissions(p, m).unwrap();
    }

    #[test]
    fn find_in_dir_requires_an_existing_file() {
        let dir = std::env::temp_dir().join(format!(
            "as-resolve-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).unwrap();
        assert!(find_in_dir(&dir, "no-such-cli-xyz").is_none());
        let f = dir.join("no-such-cli-xyz");
        fs::write(&f, "#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            // Not yet executable: must not count. On Windows there is no
            // exec bit, so the bare file is accepted as-is.
            assert!(find_in_dir(&dir, "no-such-cli-xyz").is_none());
            make_executable(&f);
        }
        assert_eq!(find_in_dir(&dir, "no-such-cli-xyz"), Some(f));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn version_key_orders_numerically() {
        assert!(version_key("v20.5.0") > version_key("v18.19.0"));
        assert_eq!(version_key("20.11.1"), vec![20, 11, 1]);
        // Numeric, not lexicographic: 11.x sorts above 9.x.
        assert!(version_key("v11.0.0") > version_key("v9.9.9"));
    }

    #[test]
    fn version_tree_prefers_the_default_alias_then_newest() {
        let root = std::env::temp_dir().join(format!(
            "as-resolve-nvm-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        for v in ["v18.0.0", "v20.5.0", "v20.11.0"] {
            fs::create_dir_all(root.join("versions/node").join(v).join("bin")).unwrap();
        }
        let versions = root.join("versions/node");
        // No alias file: newest first.
        let bins = node_version_bins(&versions, "bin", None);
        let names: Vec<String> = bins.iter().map(|p| p.to_string_lossy().into_owned()).collect();
        assert!(names[0].contains("v20.11.0"), "{names:?}");
        assert!(names[1].contains("v20.5.0"), "{names:?}");
        // Exact alias match wins over newer versions.
        let alias = root.join("alias/default");
        fs::create_dir_all(alias.parent().unwrap()).unwrap();
        fs::write(&alias, "v18.0.0\n").unwrap();
        let bins = node_version_bins(&versions, "bin", Some(&alias));
        assert!(bins[0].to_string_lossy().contains("v18.0.0"), "{bins:?}");
        // Prefix alias ("20" → any v20.x, newest of those).
        fs::write(&alias, "20").unwrap();
        let bins = node_version_bins(&versions, "bin", Some(&alias));
        assert!(bins[0].to_string_lossy().contains("v20.11.0"), "{bins:?}");
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn parse_path_var_rejects_non_path_output() {
        let ok = parse_path_var("some banner\n/usr/bin:/bin:/usr/local/bin\n");
        assert_eq!(
            ok,
            Some(vec![
                PathBuf::from("/usr/bin"),
                PathBuf::from("/bin"),
                PathBuf::from("/usr/local/bin")
            ])
        );
        // A relative entry disqualifies the whole line (profile chatter).
        assert!(parse_path_var("hello\n/usr/bin:bin").is_none());
        assert!(parse_path_var("").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn resolve_falls_back_to_the_login_shell_path() {
        // A fake "shell": a POSIX script that ignores its flags and prints
        // a PATH pointing at a temp dir holding a unique fake binary.
        let root = std::env::temp_dir().join(format!(
            "as-resolve-shell-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).unwrap();
        let bin_dir = root.join("fakedir");
        fs::create_dir_all(&bin_dir).unwrap();
        let fake = bin_dir.join("as-fake-cli-zzz");
        fs::write(&fake, "#!/bin/sh\n").unwrap();
        make_executable(&fake);
        let script = root.join("shell.sh");
        fs::write(&script, format!("#!/bin/sh\nprintf '%s' '{0}'\n", bin_dir.display())).unwrap();
        make_executable(&script);

        std::env::set_var("SHELL", &script);
        clear_binary_cache();
        assert_eq!(resolve_binary("as-fake-cli-zzz"), Some(fake.clone()));
        // Cached: still resolves after the fake shell is deleted.
        let _ = fs::remove_file(&script);
        assert_eq!(resolve_binary("as-fake-cli-zzz"), Some(fake.clone()));
        clear_binary_cache();
        assert!(resolve_binary("as-fake-cli-zzz").is_none());
        std::env::remove_var("SHELL");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn negative_results_are_cached_and_cleared() {
        clear_binary_cache();
        let name = "as-definitely-not-installed-xyz";
        assert!(resolve_binary(name).is_none());
        assert!(resolve_binary(name).is_none());
        clear_binary_cache();
        assert!(resolve_binary(name).is_none());
    }
}
