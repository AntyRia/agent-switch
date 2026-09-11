//! Cross-platform terminal detection and the "open this start script in a
//! terminal" logic used by the GUI (and available to any caller).
//!
//! Default behaviour is auto-detect: the first terminal from the per-OS
//! preference list that is on PATH. The user can pin a specific terminal
//! in the settings (an absolute path); it is matched against the known
//! recipes by file name, and a path with no known recipe falls back to
//! the auto-detect list (the caller surfaces that fallback as a warning).
//!
//! Secret env pairs (the per-launch API key) never touch the start script
//! on disk (spec §9): on Windows they are set on the terminal process
//! environment; on macOS/Linux they are exported inline in the launch
//! shell command / AppleScript payload (in-memory only).

use std::path::Path;

use crate::error::{Error, Result};
use crate::launcher::sanitized_child_env;

/// One terminal known to the launcher.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalInfo {
    /// Display name (GUI dropdown).
    pub label: String,
    /// Binary name as looked up on PATH.
    pub bin: String,
    /// Resolved absolute path (the user's custom path when pinned).
    pub path: String,
}

/// Outcome of a successful terminal open.
pub struct TerminalOutcome {
    /// Label of the terminal that was actually used.
    pub label: String,
    /// True when the user's configured terminal was not usable and the
    /// launcher fell back to an auto-detected one.
    pub used_fallback: bool,
}

/// The per-OS preference list (auto-detect order).
fn candidates() -> Vec<(&'static str, &'static str)> {
    if cfg!(windows) {
        vec![
            ("wt", "Windows Terminal"),
            ("powershell", "Windows PowerShell"),
            ("cmd", "Command Prompt"),
            ("alacritty", "Alacritty"),
            ("kitty", "Kitty"),
            ("wezterm", "WezTerm"),
            ("conemu", "ConEmu"),
        ]
    } else if cfg!(target_os = "macos") {
        vec![
            ("terminal", "Terminal"),
            ("iterm2", "iTerm2"),
            ("alacritty", "Alacritty"),
            ("kitty", "Kitty"),
            ("wezterm", "WezTerm"),
        ]
    } else {
        vec![
            ("x-terminal-emulator", "Default (x-terminal-emulator)"),
            ("gnome-terminal", "GNOME Terminal"),
            ("konsole", "Konsole"),
            ("xfce4-terminal", "xfce4-terminal"),
            ("alacritty", "Alacritty"),
            ("kitty", "Kitty"),
            ("wezterm", "WezTerm"),
            ("xterm", "XTERM"),
        ]
    }
}

/// All known terminals found on this machine, in preference order.
pub fn detect_terminals() -> Vec<TerminalInfo> {
    let mut out = Vec::new();
    for (bin, label) in candidates() {
        if let Ok(path) = which::which(bin) {
            out.push(TerminalInfo {
                label: label.to_string(),
                bin: bin.to_string(),
                path: path.to_string_lossy().into_owned(),
            });
        }
    }
    out
}

/// Match a user-configured terminal path against the known recipes by
/// file name (extension- and, on Windows, case-insensitive).
pub fn resolve_custom(path: &str) -> Option<TerminalInfo> {
    let p = Path::new(path);
    let file_name = p.file_name()?.to_str()?.to_string();
    let lower = if cfg!(windows) {
        file_name.to_ascii_lowercase()
    } else {
        file_name.clone()
    };
    let stem = match p.extension().and_then(|e| e.to_str()) {
        Some(ext) if lower.ends_with(&format!(".{}", ext.to_ascii_lowercase())) => {
            lower[..lower.len() - ext.len() - 1].to_string()
        }
        _ => lower,
    };
    let (bin, label) = candidates().into_iter().find(|(b, _)| *b == stem)?;
    Some(TerminalInfo {
        label: label.to_string(),
        bin: bin.to_string(),
        path: path.to_string(),
    })
}

/// Open `script` (a start script generated in the runtime dir) in a
/// terminal window, with the secret `env` pairs injected per the platform
/// strategy described in the module docs.
///
/// `custom_terminal` is the user's pinned terminal path ("" = auto-detect).
/// Returns the label of the terminal actually used plus whether a fallback
/// happened (the GUI turns that into a warning).
pub fn open_in_terminal(
    script: &Path,
    workspace: &Path,
    env: &[(String, String)],
    custom_terminal: &str,
) -> Result<TerminalOutcome> {
    // Candidate order: the pinned terminal first (when it matches a known
    // recipe), then the auto-detect list.
    let custom = custom_terminal.trim();
    let pinned: Option<TerminalInfo> = if custom.is_empty() {
        None
    } else {
        resolve_custom(custom)
    };
    let mut candidates: Vec<TerminalInfo> = Vec::new();
    if let Some(t) = &pinned {
        candidates.push(t.clone());
    }
    for t in detect_terminals() {
        if !candidates.iter().any(|c| c.bin == t.bin) {
            candidates.push(t);
        }
    }

    for (i, info) in candidates.iter().enumerate() {
        let fallback = pinned.is_some() && i != 0;
        match spawn(info, script, workspace, env) {
            Ok(()) => {
                return Ok(TerminalOutcome {
                    label: info.label.clone(),
                    used_fallback: fallback,
                })
            }
            Err(e) => {
                crate::logging::warn(&format!(
                    "terminal '{}' failed to start: {e}",
                    info.label
                ));
            }
        }
    }
    Err(Error::Other(format!(
        "no terminal emulator available; run this script manually: {}",
        script.display()
    )))
}

/// Spawn one terminal per its recipe.
fn spawn(info: &TerminalInfo, script: &Path, workspace: &Path, env: &[(String, String)]) -> Result<()> {
    let script = script.to_string_lossy().into_owned();
    let ws = workspace.to_string_lossy().into_owned();
    let program = if info.path.is_empty() {
        info.bin.clone()
    } else {
        info.path.clone()
    };
    let recipe = info
        .bin
        .rsplit('/')
        .next()
        .unwrap_or(&info.bin)
        .rsplit('\\')
        .next()
        .unwrap_or(&info.bin)
        .to_ascii_lowercase();

    if cfg!(windows) {
        // PowerShell start script; secrets go on the terminal process env
        // (the terminal passes its environment through to the shell).
        // The shell the terminal should run: powershell on the start
        // script (keeps the window open, bypasses execution policy).
        let ps: Vec<String> = vec![
            "powershell".into(),
            "-NoExit".into(),
            "-ExecutionPolicy".into(),
            "Bypass".into(),
            "-File".into(),
            script.clone(),
        ];
        let args: Vec<String> = match recipe.as_str() {
            "wt" => {
                // wt: `-d` sets the dir, the rest are the shell args.
                let mut a = vec!["-d".to_string(), ws.clone()];
                a.extend(ps.iter().cloned());
                a
            }
            "alacritty" | "kitty" => {
                let mut a = vec!["-d".to_string(), ws.clone(), "--".to_string()];
                a.extend(ps.iter().cloned());
                a
            }
            "powershell" => ps,
            "cmd" => {
                let mut a = vec!["/C".to_string(), "start".to_string(), String::new()];
                a.extend(ps.iter().cloned());
                a
            }
            "wezterm" => {
                let mut a = vec!["cli".to_string(), "start".to_string(), "--cwd".into(), ws.clone()];
                a.push("--".into());
                a.extend(ps.iter().cloned());
                a
            }
            "conemu" => {
                let mut a = vec!["-ExecFromCommandLine".to_string()];
                a.extend(ps.iter().cloned());
                a
            }
            other => {
                return Err(Error::Other(format!(
                    "no launch recipe for terminal '{other}' on Windows"
                )))
            }
        };
        // Rebuild the environment: the sanitized parent env + the launch
        // pairs. The parent env may carry Claude Code nested-session
        // markers (GUI started from inside a Claude Code session), which
        // the launched CLI must not see.
        let mut pairs = sanitized_child_env();
        pairs.extend(env.iter().cloned());
        let mut cmd = std::process::Command::new(&program);
        for a in &args {
            cmd.arg(a);
        }
        if recipe == "powershell" || recipe == "conemu" {
            cmd.current_dir(&ws);
        }
        cmd.env_clear();
        cmd.envs(pairs);
        cmd.spawn().map_err(|e| {
            Error::Other(format!("failed to open terminal '{}': {e}", info.label))
        })?;
        crate::logging::info(&format!("opened terminal: {} ({})", info.label, program));
        Ok(())
    } else {
        // Unix/macOS: terminal emulators do not pass the caller's
        // environment through, so the secrets are exported inline in the
        // launch payload (in-memory only, never written to the script).
        let exports = env
            .iter()
            .map(|(k, v)| format!("export {k}='{}'", sh_quote(v)))
            .collect::<Vec<_>>()
            .join("; ");
        let shell_cmd = format!("{}; exec '{}'", exports, sh_quote(&script));
        let payload =
            format!("{exports}; source '{}'", sh_quote(&script));
        let mut cmd = match recipe.as_str() {
            "terminal" => {
                let mut c = std::process::Command::new("osascript");
                c.arg("-e")
                    .arg(format!(
                        "tell application \"Terminal\" to do script \"{}\"",
                        applescript_quote(&payload)
                    ));
                c
            }
            "iterm2" => {
                let mut c = std::process::Command::new("osascript");
                c.arg("-e").arg(format!(
                    "tell application \"iTerm\"\nactivate\nset newWindow to (create window with default profile)\ntell current session of newWindow to write text \"{}\"\nend tell",
                    applescript_quote(&payload)
                ));
                c
            }
            "gnome-terminal" => {
                let mut c = std::process::Command::new(&program);
                c.arg("--").arg("sh").arg("-c").arg(&shell_cmd);
                c
            }
            "konsole" | "xterm" => {
                let mut c = std::process::Command::new(&program);
                c.arg("-e").arg("sh").arg("-c").arg(&shell_cmd);
                c
            }
            "xfce4-terminal" => {
                let mut c = std::process::Command::new(&program);
                c.arg("-x").arg("sh").arg("-c").arg(&shell_cmd);
                c
            }
            "alacritty" | "kitty" => {
                let mut c = std::process::Command::new(&program);
                c.arg("-d").arg(&ws).arg("--").arg("sh").arg("-c").arg(&shell_cmd);
                c
            }
            "wezterm" => {
                let mut c = std::process::Command::new(&program);
                c.arg("cli")
                    .arg("start")
                    .arg("--cwd")
                    .arg(&ws)
                    .arg("--")
                    .arg("sh")
                    .arg("-c")
                    .arg(&shell_cmd);
                c
            }
            // x-terminal-emulator and anything else: bare wrapper.
            _ => {
                let mut c = std::process::Command::new(&program);
                c.arg("sh").arg("-c").arg(&shell_cmd);
                c
            }
        };
        cmd.spawn().map_err(|e| {
            Error::Other(format!("failed to open terminal '{}': {e}", info.label))
        })?;
        crate::logging::info(&format!("opened terminal: {}", info.label));
        Ok(())
    }
}

/// Shell single-quote escaping: `'` -> `'\''`.
fn sh_quote(s: &str) -> String {
    s.replace('\'', "'\\''")
}

/// Escape a string for inclusion in an AppleScript string literal.
fn applescript_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_returns_known_terminals_in_order() {
        let found = detect_terminals();
        // At least the per-OS default must exist on a dev machine
        // (Windows: powershell is always there; macOS: Terminal; Linux:
        // usually at least xterm or x-terminal-emulator).
        assert!(!found.is_empty());
        // Labels are unique.
        let labels: std::collections::HashSet<_> =
            found.iter().map(|t| t.label.clone()).collect();
        assert_eq!(labels.len(), found.len());
    }

    #[test]
    fn resolve_custom_matches_by_file_name() {
        let stem = if cfg!(windows) {
            r"C:\Program Files\WindowsApps\wt.exe"
        } else {
            "/usr/bin/gnome-terminal"
        };
        let info = resolve_custom(stem);
        assert!(info.is_some());
        assert_eq!(
            info.map(|i| i.bin),
            if cfg!(windows) {
                Some("wt".to_string())
            } else {
                Some("gnome-terminal".to_string())
            }
        );
        assert!(resolve_custom("/no/such/thing-xyz").is_none());
    }

    #[test]
    fn sh_quote_escapes_single_quotes() {
        assert_eq!(sh_quote("a'b"), "a'\\''b");
        assert_eq!(sh_quote("plain"), "plain");
    }

    #[test]
    fn applescript_quote_doubles_backslash_and_quote() {
        // a \ " b → a \\ \" b (3 backslashes + quote in the result).
        assert_eq!(applescript_quote(r#"a\"b"#), "a\\\\\\\"b");
        assert_eq!(applescript_quote("ok"), "ok");
    }
}
