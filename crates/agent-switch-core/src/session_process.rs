//! Only agent processes can own transcripts. Terminal hosts and shell command
//! lines can retain a session id long after its tab or CLI has exited.

#[derive(Debug, Clone)]
pub(crate) struct Process {
    pub pid: u32,
    pub name: String,
    pub start: Option<String>,
    pub command: Option<String>,
}

pub(crate) struct Snapshot {
    pub processes: Vec<Process>,
    pub complete: bool,
}

pub(crate) fn is_agent(name: &str) -> bool {
    let name = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(name)
        .to_ascii_lowercase();
    matches!(name.as_str(), "claude" | "codex" | "node" | "node.exe")
        || name.starts_with("claude.exe")
        || name.starts_with("codex.exe")
}

/// Preserve quoted paths and backslashes; only actual CLI argument values
/// identify a session. A prompt mentioning an id is never an ownership claim.
fn words(command: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    for ch in command.chars() {
        if Some(ch) == quote {
            quote = None;
        } else if quote.is_none() && matches!(ch, '\'' | '"') {
            quote = Some(ch);
        } else if quote.is_none() && ch.is_whitespace() {
            if !word.is_empty() {
                args.push(std::mem::take(&mut word));
            }
        } else {
            word.push(ch);
        }
    }
    if !word.is_empty() {
        args.push(word);
    }
    args
}

pub(crate) fn claimed_sessions(process: &Process) -> Vec<String> {
    if !is_agent(&process.name) {
        return Vec::new();
    }
    let args = words(process.command.as_deref().unwrap_or_default());
    let name = process.name.to_ascii_lowercase();
    if matches!(name.as_str(), "node" | "node.exe")
        && !args.get(1).is_some_and(|s| {
            let s = s.replace('\\', "/").to_ascii_lowercase();
            s.contains("/claude-code/") || s.contains("/codex/")
        })
    {
        return Vec::new();
    }
    let value = |flag: &str| -> Option<String> {
        args.iter().enumerate().find_map(|(i, arg)| {
            arg.strip_prefix(&format!("{flag}="))
                .map(str::to_string)
                .or_else(|| (arg == flag).then(|| args.get(i + 1).cloned()).flatten())
        })
    };
    // A fork reads its source transcript but writes to its own new session.
    if let Some(id) = value("--session-id") {
        return vec![id];
    }
    if args.iter().any(|a| a == "--fork-session") {
        return Vec::new();
    }
    let id = value("--resume").or_else(|| value("resume"));
    id.map(|id| {
        let id = id.rsplit(['/', '\\']).next().unwrap_or(&id);
        vec![id.strip_suffix(".jsonl").unwrap_or(id).to_string()]
    })
    .unwrap_or_default()
}

#[cfg(windows)]
pub(crate) fn snapshot() -> Snapshot {
    use std::ffi::c_void;
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    #[link(name = "ntdll")]
    extern "system" {
        fn NtQueryInformationProcess(
            handle: *mut c_void,
            class: u32,
            buffer: *mut c_void,
            length: u32,
            returned: *mut u32,
        ) -> i32;
    }
    #[repr(C)]
    struct UnicodeString {
        length: u16,
        maximum: u16,
        buffer: *const u16,
    }
    let mut result = Snapshot {
        processes: Vec::new(),
        complete: true,
    };
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE || snap.is_null() {
            result.complete = false;
            return result;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut entry);
        if ok == 0 {
            result.complete = false;
        }
        while ok != 0 {
            let end = entry
                .szExeFile
                .iter()
                .position(|ch| *ch == 0)
                .unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..end]);
            if is_agent(&name) {
                let mut process = Process {
                    pid: entry.th32ProcessID,
                    name,
                    start: None,
                    command: None,
                };
                let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process.pid);
                if !handle.is_null() {
                    let mut creation: FILETIME = std::mem::zeroed();
                    let mut exit: FILETIME = std::mem::zeroed();
                    let mut kernel: FILETIME = std::mem::zeroed();
                    let mut user: FILETIME = std::mem::zeroed();
                    if GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user)
                        != 0
                    {
                        process.start = Some(
                            (((creation.dwHighDateTime as u64) << 32)
                                | creation.dwLowDateTime as u64)
                                .to_string(),
                        );
                    }
                    // ProcessCommandLineInformation returns a local UNICODE_STRING.
                    // This avoids undocumented PEB offsets and preserves Chinese paths.
                    let mut length = 0;
                    NtQueryInformationProcess(handle, 60, std::ptr::null_mut(), 0, &mut length);
                    if length > 0 && length <= 131072 {
                        let mut buffer =
                            vec![0usize; (length as usize).div_ceil(std::mem::size_of::<usize>())];
                        if NtQueryInformationProcess(
                            handle,
                            60,
                            buffer.as_mut_ptr().cast(),
                            length,
                            &mut length,
                        ) == 0
                        {
                            let text = &*buffer.as_ptr().cast::<UnicodeString>();
                            let begin = buffer.as_ptr() as usize;
                            let end = begin + buffer.len() * std::mem::size_of::<usize>();
                            let address = text.buffer as usize;
                            if text.length % 2 == 0
                                && address >= begin
                                && address.saturating_add(text.length as usize) <= end
                            {
                                process.command =
                                    Some(String::from_utf16_lossy(std::slice::from_raw_parts(
                                        text.buffer,
                                        text.length as usize / 2,
                                    )));
                            }
                        }
                    }
                    CloseHandle(handle);
                }
                if process.command.is_none() {
                    result.complete = false;
                }
                result.processes.push(process);
            }
            ok = Process32NextW(snap, &mut entry);
        }
        CloseHandle(snap);
    }
    result
}

#[cfg(not(windows))]
pub(crate) fn snapshot() -> Snapshot {
    // macOS comm may contain a full executable path with spaces. ucomm is
    // the basename, keeping the first two columns unambiguous.
    #[cfg(target_os = "macos")]
    let columns = "pid=,ucomm=,args=";
    #[cfg(not(target_os = "macos"))]
    let columns = "pid=,comm=,args=";
    let output = std::process::Command::new("ps")
        .args(["-eo", columns])
        .output();
    let Ok(output) = output else {
        return Snapshot {
            processes: Vec::new(),
            complete: false,
        };
    };
    if !output.status.success() {
        return Snapshot {
            processes: Vec::new(),
            complete: false,
        };
    }
    let processes = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse().ok()?;
            let name = fields.next()?.to_string();
            if !is_agent(&name) {
                return None;
            }
            #[cfg(target_os = "linux")]
            let start = std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .ok()
                .and_then(|s| {
                    s.rsplit_once(')')
                        .and_then(|(_, rest)| rest.split_whitespace().nth(19))
                        .map(str::to_string)
                });
            #[cfg(not(target_os = "linux"))]
            let start = None;
            Some(Process {
                pid,
                name,
                start,
                command: Some(fields.collect::<Vec<_>>().join(" ")),
            })
        })
        .collect();
    Snapshot {
        processes,
        complete: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn claims(name: &str, command: &str) -> Vec<String> {
        claimed_sessions(&Process {
            pid: 1,
            name: name.into(),
            start: None,
            command: Some(command.into()),
        })
    }
    #[test]
    fn hosts_prompts_and_fork_sources_never_own_a_session() {
        for name in ["WindowsTerminal.exe", "powershell.exe", "cmd.exe"] {
            assert!(claims(name, "host --resume original").is_empty());
        }
        assert!(claims("claude.exe", "claude --fork-session --resume original").is_empty());
        assert_eq!(
            claims(
                "claude.exe",
                "claude --session-id new --fork-session --resume original"
            ),
            ["new"]
        );
        assert!(claims("claude.exe", "claude 'please explain original'").is_empty());
        assert!(claims("node.exe", "node unrelated.js --resume original").is_empty());
    }
    #[test]
    fn session_arguments_support_quoted_unicode_paths_and_equal_flags() {
        assert_eq!(
            claims(
                "claude.exe",
                r#""C:\中文 路径\claude.exe" --resume "D:\会话 记录\original.jsonl""#
            ),
            ["original"]
        );
        assert_eq!(claims("claude", "claude --resume=original"), ["original"]);
        assert_eq!(claims("codex.exe", "codex resume original"), ["original"]);
        assert_eq!(
            claims(
                "node.exe",
                "node /npm/@anthropic-ai/claude-code/cli.js --resume original"
            ),
            ["original"]
        );
        assert!(claims("claude.exe", "claude attach original").is_empty());
    }
}
