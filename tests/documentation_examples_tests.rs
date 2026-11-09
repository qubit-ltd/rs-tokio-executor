// =============================================================================
//    Copyright (c) 2025 - 2026 Haixing Hu.
//
//    SPDX-License-Identifier: Apache-2.0
//
//    Licensed under the Apache License, Version 2.0.
// =============================================================================
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Child;
use std::process::Command;
use std::process::ExitStatus;
use std::process::Stdio;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;
use std::time::Instant;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

const DOCUMENTS: [&str; 4] = [
    "README.md",
    "README.zh_CN.md",
    "doc/user_guide.md",
    "doc/user_guide.zh_CN.md",
];

static TEMP_ID: AtomicUsize = AtomicUsize::new(0);
const CHILD_MODE: &str = "QUBIT_DOC_EXAMPLE_WATCHDOG_CHILD";
const RUN_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_BUILD_TIMEOUT: Duration = Duration::from_secs(600);
const BUILD_TIMEOUT_ENV: &str = "QUBIT_DOC_EXAMPLE_BUILD_TIMEOUT_SECS";
const POLL_INTERVAL: Duration = Duration::from_millis(10);
#[cfg(windows)]
const CREATE_SUSPENDED: u32 = 0x0000_0004;

struct CapturedOutput {
    status: Option<ExitStatus>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    timed_out: bool,
}

fn parse_build_timeout(value: Option<std::ffi::OsString>) -> Result<Duration, String> {
    let Some(value) = value else {
        return Ok(DEFAULT_BUILD_TIMEOUT);
    };
    let value = value
        .into_string()
        .map_err(|_| format!("{BUILD_TIMEOUT_ENV} must be a positive integer number of seconds"))?;
    let seconds = value
        .parse::<u64>()
        .map_err(|_| format!("{BUILD_TIMEOUT_ENV} must be a positive integer number of seconds"))?;
    if seconds == 0 {
        return Err(format!("{BUILD_TIMEOUT_ENV} must be greater than zero"));
    }
    let timeout = Duration::from_secs(seconds);
    Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| format!("{BUILD_TIMEOUT_ENV} is too large to represent as a deadline"))?;
    Ok(timeout)
}

#[cfg(windows)]
#[repr(C)]
#[derive(Default)]
struct JobObjectBasicLimitInformation {
    per_process_user_time_limit: i64,
    per_job_user_time_limit: i64,
    limit_flags: u32,
    minimum_working_set_size: usize,
    maximum_working_set_size: usize,
    active_process_limit: u32,
    affinity: usize,
    priority_class: u32,
    scheduling_class: u32,
}

#[cfg(windows)]
#[repr(C)]
#[derive(Default)]
struct JobObjectIoCounters {
    read_operation_count: u64,
    write_operation_count: u64,
    other_operation_count: u64,
    read_transfer_count: u64,
    write_transfer_count: u64,
    other_transfer_count: u64,
}

#[cfg(windows)]
#[repr(C)]
#[derive(Default)]
struct JobObjectExtendedLimitInformation {
    basic_limit_information: JobObjectBasicLimitInformation,
    io_info: JobObjectIoCounters,
    process_memory_limit: usize,
    job_memory_limit: usize,
    peak_process_memory_used: usize,
    peak_job_memory_used: usize,
}

#[cfg(windows)]
#[repr(C)]
#[derive(Default)]
struct ThreadEntry32 {
    size: u32,
    usage_count: u32,
    thread_id: u32,
    owner_process_id: u32,
    base_priority: i32,
    delta_priority: i32,
    flags: u32,
}

#[cfg(windows)]
struct WindowsJobObject {
    handle: Option<std::ptr::NonNull<std::ffi::c_void>>,
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateJobObjectW(attributes: *const std::ffi::c_void, name: *const u16) -> *mut std::ffi::c_void;
    fn SetInformationJobObject(
        job: *mut std::ffi::c_void,
        information_class: u32,
        information: *mut std::ffi::c_void,
        information_length: u32,
    ) -> i32;
    fn AssignProcessToJobObject(job: *mut std::ffi::c_void, process: *mut std::ffi::c_void) -> i32;
    fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
    fn CreateToolhelp32Snapshot(flags: u32, process_id: u32) -> *mut std::ffi::c_void;
    fn Thread32First(snapshot: *mut std::ffi::c_void, entry: *mut ThreadEntry32) -> i32;
    fn Thread32Next(snapshot: *mut std::ffi::c_void, entry: *mut ThreadEntry32) -> i32;
    fn OpenThread(access: u32, inherit_handle: i32, thread_id: u32) -> *mut std::ffi::c_void;
    fn ResumeThread(thread: *mut std::ffi::c_void) -> u32;
    fn GetLastError() -> u32;
}

#[cfg(windows)]
impl WindowsJobObject {
    fn new() -> Result<Self, String> {
        // SAFETY: null attributes/name request a default unnamed Job Object; the
        // returned handle is checked and owned by this wrapper.
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        let handle = std::ptr::NonNull::new(handle)
            .ok_or_else(|| format!("CreateJobObjectW failed: {}", std::io::Error::last_os_error()))?;
        let mut limits = JobObjectExtendedLimitInformation::default();
        limits.basic_limit_information.limit_flags = 0x0000_2000; // JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        // SAFETY: `limits` has the documented repr(C) extended-limit layout and remains
        // alive for the duration of this synchronous API call.
        let configured = unsafe {
            SetInformationJobObject(
                handle.as_ptr(),
                9, // JobObjectExtendedLimitInformation
                (&mut limits as *mut JobObjectExtendedLimitInformation).cast(),
                std::mem::size_of::<JobObjectExtendedLimitInformation>() as u32,
            )
        };
        if configured == 0 {
            let error = std::io::Error::last_os_error();
            // SAFETY: this wrapper created the handle and no process has been assigned yet.
            unsafe { CloseHandle(handle.as_ptr()) };
            return Err(format!("configure Job Object kill-on-close: {error}"));
        }
        Ok(Self { handle: Some(handle) })
    }

    fn assign(&self, child: &Child) -> Result<(), String> {
        use std::os::windows::io::AsRawHandle;
        let job = self
            .handle
            .ok_or_else(|| "Job Object handle is already closed".to_owned())?;
        // SAFETY: Child owns a live process handle while this method runs; the Job
        // Object handle is valid and owned by self.
        let assigned = unsafe { AssignProcessToJobObject(job.as_ptr(), child.as_raw_handle().cast()) };
        if assigned == 0 {
            return Err(format!(
                "AssignProcessToJobObject failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }

    fn resume_primary_thread(&self, process_id: u32) -> Result<(), String> {
        const TH32CS_SNAPTHREAD: u32 = 0x0000_0004;
        const THREAD_SUSPEND_RESUME: u32 = 0x0000_0002;
        const INVALID_HANDLE_VALUE: isize = -1;

        // SAFETY: this requests a system-owned snapshot of threads; the invalid
        // sentinel and null handle are checked before any other Toolhelp call.
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
        if snapshot.is_null() || snapshot as isize == INVALID_HANDLE_VALUE {
            return Err(format!(
                "CreateToolhelp32Snapshot failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        let mut entry = ThreadEntry32 {
            size: std::mem::size_of::<ThreadEntry32>() as u32,
            ..ThreadEntry32::default()
        };
        // SAFETY: `entry` has the documented THREADENTRY32 layout and a valid size
        // field.
        let first = unsafe { Thread32First(snapshot, &mut entry) };
        if first == 0 {
            let error = std::io::Error::last_os_error();
            let snapshot_close = close_windows_handle(snapshot, "Toolhelp thread snapshot");
            return Err(format!(
                "Thread32First failed: {error}; close snapshot: {snapshot_close:?}"
            ));
        }
        let mut found = true;
        let mut matching_thread = None;
        let mut enumeration_error = None;
        while found {
            if entry.owner_process_id == process_id {
                matching_thread = Some(entry.thread_id);
                break;
            }
            // SAFETY: snapshot remains open and `entry` remains a valid initialized buffer.
            found = unsafe { Thread32Next(snapshot, &mut entry) } != 0;
            if !found {
                // SAFETY: GetLastError reads the calling thread's status from the failed
                // Toolhelp32 enumeration call.
                let error_code = unsafe { GetLastError() };
                if error_code != 18 {
                    enumeration_error = Some(std::io::Error::from_raw_os_error(error_code as i32));
                }
            }
        }
        let snapshot_close = close_windows_handle(snapshot, "Toolhelp thread snapshot");
        if let Some(error) = enumeration_error {
            return Err(format!(
                "Thread32Next failed: {error}; close snapshot: {snapshot_close:?}"
            ));
        }
        let thread_id = matching_thread.ok_or_else(|| {
            format!("no primary thread found for suspended process {process_id}; close snapshot: {snapshot_close:?}")
        })?;

        // SAFETY: OpenThread returns an owned handle for the enumerated thread ID or
        // null.
        let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, thread_id) };
        let thread = std::ptr::NonNull::new(thread).ok_or_else(|| {
            format!(
                "OpenThread({thread_id}) failed: {}; close snapshot: {snapshot_close:?}",
                std::io::Error::last_os_error()
            )
        })?;
        // SAFETY: the owned thread handle requests THREAD_SUSPEND_RESUME access.
        let previous_suspend_count = unsafe { ResumeThread(thread.as_ptr()) };
        let resume_error = (previous_suspend_count == u32::MAX).then(std::io::Error::last_os_error);
        let thread_close = close_windows_handle(thread.as_ptr(), "primary thread");
        if let Some(error) = resume_error {
            return Err(format!(
                "ResumeThread({thread_id}) failed: {error}; close thread: {thread_close:?}; close snapshot: {snapshot_close:?}"
            ));
        }
        if previous_suspend_count == 0 {
            return Err(format!(
                "ResumeThread({thread_id}) reported a thread that was not suspended; close thread: {thread_close:?}; close snapshot: {snapshot_close:?}"
            ));
        }
        thread_close?;
        snapshot_close?;
        Ok(())
    }

    fn close(&mut self) -> Result<(), String> {
        let Some(handle) = self.handle else {
            return Ok(());
        };
        // SAFETY: this wrapper uniquely owns the handle and closes it at most once.
        if unsafe { CloseHandle(handle.as_ptr()) } == 0 {
            return Err(format!(
                "CloseHandle(Job Object) failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        self.handle = None;
        Ok(())
    }
}

#[cfg(windows)]
fn close_windows_handle(handle: *mut std::ffi::c_void, label: &str) -> Result<(), String> {
    // SAFETY: caller passes a valid owned Win32 handle and closes it exactly once.
    if unsafe { CloseHandle(handle) } == 0 {
        return Err(format!(
            "CloseHandle({label}) failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}

#[cfg(windows)]
impl Drop for WindowsJobObject {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

struct ChildGuard {
    child: Child,
    #[cfg(unix)]
    process_group: u32,
    #[cfg(windows)]
    process_id: u32,
    #[cfg(windows)]
    windows_job: WindowsJobObject,
    #[cfg(not(any(unix, windows)))]
    unsupported: (),
}

impl ChildGuard {
    fn terminate_tree(&mut self, label: &str) -> Result<Option<ExitStatus>, String> {
        #[cfg(unix)]
        {
            let group = format!("-{}", self.process_group);
            let killed = Command::new("kill")
                .args(["-KILL", "--", group.as_str()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map_err(|error| format!("kill process group {} for {label}: {error}", self.process_group))?;
            if !killed.success() {
                let exists = Command::new("kill")
                    .args(["-0", "--", group.as_str()])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .map_err(|error| format!("check process group {} for {label}: {error}", self.process_group))?;
                if exists.success() {
                    return Err(format!(
                        "kill process group {} for {label} exited with {killed}",
                        self.process_group
                    ));
                }
            }
        }
        #[cfg(windows)]
        {
            let root_running = self
                .child
                .try_wait()
                .map_err(|error| format!("poll root process {} for {label}: {error}", self.process_id))?
                .is_none();
            let taskkill_result = if root_running {
                Some(
                    Command::new("taskkill")
                        .args(["/PID", &self.process_id.to_string(), "/T", "/F"])
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status(),
                )
            } else {
                None
            };
            self.windows_job.close().map_err(|error| {
                format!(
                    "close process Job Object {} for {label}: {error}; taskkill attempt: {taskkill_result:?}",
                    self.process_id
                )
            })?;
        }
        #[cfg(not(any(unix, windows)))]
        return Err(format!(
            "process-tree watchdog is unsupported on this platform ({label})"
        ));
        self.child
            .wait()
            .map(Some)
            .map_err(|error| format!("wait for {label}: {error}"))
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        // The root may already have exited while descendants remain in its tree.
        let _ = self.terminate_tree("watchdog child drop");
    }
}

fn run_with_watchdog(mut command: Command, timeout: Duration, label: &str) -> Result<CapturedOutput, String> {
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| format!("watchdog deadline for {label} is not representable: {timeout:?}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(not(any(unix, windows)))]
    return Err(format!(
        "process-tree watchdog is unsupported on this platform ({label})"
    ));
    let logs = TempDir::new("watchdog-logs");
    let stdout_path = logs.0.join("stdout");
    let stderr_path = logs.0.join("stderr");
    let stdout_file = fs::File::create(&stdout_path).map_err(|error| format!("create stdout log: {error}"))?;
    let stderr_file = fs::File::create(&stderr_path).map_err(|error| format!("create stderr log: {error}"))?;
    command
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file));
    #[cfg(windows)]
    let windows_job =
        WindowsJobObject::new().map_err(|error| format!("create process Job Object for {label}: {error}"))?;
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_SUSPENDED);
    }
    let child = command.spawn().map_err(|error| format!("spawn {label}: {error}"))?;
    #[cfg(unix)]
    let mut child = ChildGuard {
        process_group: child.id(),
        child,
    };
    #[cfg(windows)]
    let mut child = ChildGuard {
        process_id: child.id(),
        windows_job,
        child,
    };
    #[cfg(not(any(unix, windows)))]
    let mut child = ChildGuard { unsupported: (), child };
    #[cfg(windows)]
    if let Err(error) = child.windows_job.assign(&child.child) {
        let taskkill = Command::new("taskkill")
            .args(["/PID", &child.process_id.to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let direct_kill = child.child.kill();
        let waited = child.child.wait();
        return Err(watchdog_cleanup_diagnostic(
            label,
            &stdout_path,
            &stderr_path,
            format!(
                "assign root to Job Object failed: {error}; taskkill={taskkill:?}; direct kill={direct_kill:?}; wait={waited:?}"
            ),
        ));
    }
    #[cfg(windows)]
    if let Err(error) = child.windows_job.resume_primary_thread(child.process_id) {
        let cleanup = child.terminate_tree(label);
        return Err(watchdog_cleanup_diagnostic(
            label,
            &stdout_path,
            &stderr_path,
            format!("resume suspended root failed: {error}; cleanup: {cleanup:?}"),
        ));
    }
    let (status, timed_out) = loop {
        match child.child.try_wait() {
            Ok(Some(status)) => break (Some(status), false),
            Ok(None) if Instant::now() < deadline => thread::sleep(POLL_INTERVAL),
            Ok(None) => match child.terminate_tree(label) {
                Ok(status) => break (status, true),
                Err(error) => return Err(watchdog_cleanup_diagnostic(label, &stdout_path, &stderr_path, error)),
            },
            Err(error) => {
                let cleanup = child.terminate_tree(label);
                return Err(watchdog_cleanup_diagnostic(
                    label,
                    &stdout_path,
                    &stderr_path,
                    format!("poll {label}: {error}; cleanup: {cleanup:?}"),
                ));
            }
        }
    };
    if let Err(error) = child.terminate_tree(label) {
        return Err(watchdog_cleanup_diagnostic(
            label,
            &stdout_path,
            &stderr_path,
            format!("status={:?}; {error}", status.and_then(|status| status.code())),
        ));
    }
    let stdout = fs::read(&stdout_path).map_err(|error| format!("read stdout log: {error}"))?;
    let stderr = fs::read(&stderr_path).map_err(|error| format!("read stderr log: {error}"))?;
    Ok(CapturedOutput {
        status,
        stdout,
        stderr,
        timed_out,
    })
}

fn watchdog_cleanup_diagnostic(label: &str, stdout: &Path, stderr: &Path, error: String) -> String {
    let stdout = fs::read(stdout).unwrap_or_default();
    let stderr = fs::read(stderr).unwrap_or_default();
    format!(
        "{label}: {error}; status=unavailable; stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&stdout),
        String::from_utf8_lossy(&stderr),
    )
}

fn output_diagnostic(program: &CodeBlock, name: &str, result: &CapturedOutput, timeout: Duration) -> String {
    format!(
        "{}:{}-{} ({name}) {} after {timeout:?}; status={:?}; stdout:\n{}\nstderr:\n{}",
        program.document,
        program.start_line,
        program.end_line,
        if result.timed_out { "timed out" } else { "failed" },
        result.status.and_then(|status| status.code()),
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr),
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Dependency {
    version: String,
    features: Vec<String>,
}

#[derive(Debug, Clone)]
struct CodeBlock {
    document: String,
    start_line: usize,
    end_line: usize,
    source: String,
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock should be after Unix epoch")
            .as_nanos();
        let id = TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "tokio-executor-docs-{label}-{}-{timestamp}-{id}",
            std::process::id(),
        ));
        fs::create_dir_all(&path).expect("temporary directory should be created");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn read_document(root: &Path, relative: &str) -> String {
    fs::read_to_string(root.join(relative)).unwrap_or_else(|error| panic!("failed to read {relative}: {error}"))
}

/// Extracts every complete Rust fenced block and fails instead of silently
/// skipping an empty document. Partial snippets must use the explicit
/// `rust,fragment` info string; unknown Rust fence labels are errors.
fn extract_programs(document: &str, source: &str) -> Result<Vec<CodeBlock>, String> {
    let lines: Vec<&str> = document.lines().collect();
    let mut blocks = Vec::new();
    let mut cursor = 0;
    while cursor < lines.len() {
        let line = lines[cursor].trim_start();
        if !line.starts_with("```") {
            cursor += 1;
            continue;
        }
        let info = line[3..].trim();
        // Report the first content line, matching the fenced TOML extractor.
        let start = cursor + 2;
        cursor += 1;
        let mut body = Vec::new();
        let mut closed = false;
        while cursor < lines.len() {
            if lines[cursor].trim_start().starts_with("```") {
                closed = true;
                break;
            }
            body.push(lines[cursor]);
            cursor += 1;
        }
        if !closed {
            return Err(format!("{source}:{start}: unterminated fenced block"));
        }
        let end = cursor + 1;
        if info == "rust" {
            let normalized = normalize_code(&body.join("\n"));
            if normalized.is_empty() {
                return Err(format!("{source}:{start}-{end}: empty Rust code block"));
            }
            blocks.push(CodeBlock {
                document: source.to_owned(),
                start_line: start,
                end_line: end,
                source: normalized,
            });
        } else if info.starts_with("rust") && info != "rust,fragment" {
            return Err(format!("{source}:{start}-{end}: unsupported Rust fence label {info:?}"));
        }
        cursor += 1;
    }
    if blocks.is_empty() {
        return Err(format!("{source}: no complete `rust` program fences found"));
    }
    Ok(blocks)
}

fn normalize_code(source: &str) -> String {
    source
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

/// Parses a TOML installation snippet and validates the dependency fields used
/// by the project documentation.
fn parse_install_toml(source: &str) -> Result<BTreeMap<String, Dependency>, String> {
    let document: toml::Value = toml::from_str(source).map_err(|error| error.to_string())?;
    let root = document
        .as_table()
        .ok_or_else(|| "TOML document must be a table".to_owned())?;
    for key in root.keys() {
        if key != "dependencies" {
            return Err(format!("unsupported root field {key:?}; expected only `dependencies`"));
        }
    }
    let dependency_table = root
        .get("dependencies")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| "missing or invalid `dependencies` table".to_owned())?;
    if dependency_table.is_empty() {
        return Err("`dependencies` table is empty".to_owned());
    }

    let mut dependencies = BTreeMap::new();
    for (name, value) in dependency_table {
        let dependency = if let Some(version) = value.as_str() {
            if version.is_empty() {
                return Err(format!("dependency {name:?} field `version` must not be empty"));
            }
            Dependency {
                version: version.to_owned(),
                features: Vec::new(),
            }
        } else if let Some(fields) = value.as_table() {
            for field in fields.keys() {
                if field != "version" && field != "features" {
                    return Err(format!("dependency {name:?} has unsupported field {field:?}"));
                }
            }
            let version = fields
                .get("version")
                .and_then(toml::Value::as_str)
                .filter(|version| !version.is_empty())
                .ok_or_else(|| format!("dependency {name:?} field `version` must be a non-empty string"))?;
            let features = match fields.get("features") {
                None => Vec::new(),
                Some(value) => value
                    .as_array()
                    .ok_or_else(|| format!("dependency {name:?} field `features` must be an array of strings"))?
                    .iter()
                    .enumerate()
                    .map(|(index, feature)| {
                        feature
                            .as_str()
                            .filter(|feature| !feature.is_empty())
                            .map(str::to_owned)
                            .ok_or_else(|| {
                                format!("dependency {name:?} field `features` item {index} must be a non-empty string")
                            })
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            };
            Dependency {
                version: version.to_owned(),
                features,
            }
        } else {
            return Err(format!(
                "dependency {name:?} must be a version string or a table with `version` and optional `features`"
            ));
        };
        dependencies.insert(name.clone(), dependency);
    }
    Ok(dependencies)
}

fn extract_toml_blocks(document: &str, source: &str) -> Result<Vec<(usize, String)>, String> {
    let lines: Vec<&str> = document.lines().collect();
    let mut blocks = Vec::new();
    let mut cursor = 0;
    while cursor < lines.len() {
        let line = lines[cursor].trim_start();
        if !line.starts_with("```") {
            cursor += 1;
            continue;
        }
        let info = line[3..].trim();
        let start_line = cursor + 1;
        cursor += 1;
        let mut body = Vec::new();
        let mut closed = false;
        while cursor < lines.len() {
            if lines[cursor].trim_start().starts_with("```") {
                closed = true;
                break;
            }
            body.push(lines[cursor]);
            cursor += 1;
        }
        if !closed {
            return Err(format!("{source}:{start_line}: unterminated fenced block"));
        }
        if info == "toml" {
            blocks.push((start_line, body.join("\n")));
        }
        cursor += 1;
    }
    if blocks.is_empty() {
        return Err(format!("{source}: no TOML installation block found"));
    }
    Ok(blocks)
}

fn validate_document_dependencies(
    document: &str,
    source: &str,
    package_version: &str,
    manifest_source: &str,
) -> Result<BTreeMap<String, Dependency>, String> {
    let blocks = extract_toml_blocks(document, source)?;
    let mut expected: Option<BTreeMap<String, Dependency>> = None;
    for (line, block) in blocks {
        let parsed =
            parse_install_toml(&block).map_err(|error| format!("{source}:{line}: invalid install TOML: {error}"))?;
        let tokio_executor = parsed
            .get("qubit-tokio-executor")
            .ok_or_else(|| format!("{source}:{line}: missing qubit-tokio-executor dependency"))?;
        let version_parts: Vec<&str> = package_version.split('.').collect();
        let expected_series = format!("{}.{}", version_parts[0], version_parts[1]);
        if tokio_executor.version != expected_series {
            return Err(format!(
                "{source}:{line}: qubit-tokio-executor must use {expected_series}, found {}",
                tokio_executor.version
            ));
        }
        for (name, display_name) in [("qubit-executor", "qubit-executor"), ("tokio", "Tokio")] {
            let dependency = parsed
                .get(name)
                .ok_or_else(|| format!("{source}:{line}: missing {display_name} dependency"))?;
            let manifest_requirement = manifest_dependency_requirement(manifest_source, name)?;
            validate_dependency_requirement(&manifest_requirement, dependency, name)
                .map_err(|error| format!("{source}:{line}: {error}"))?;
        }
        let tokio = parsed.get("tokio").expect("Tokio dependency was validated");
        for feature in ["macros", "rt-multi-thread", "time"] {
            if !tokio.features.iter().any(|item| item == feature) {
                return Err(format!("{source}:{line}: Tokio dependency missing `{feature}` feature"));
            }
        }
        if let Some(previous) = &expected {
            if previous != &parsed {
                return Err(format!("{source}:{line}: install dependency snippets disagree"));
            }
        } else {
            expected = Some(parsed);
        }
    }
    expected.ok_or_else(|| format!("{source}: no parsed install dependencies"))
}

fn manifest_dependency_requirement(manifest_source: &str, name: &str) -> Result<String, String> {
    let manifest: toml::Value = toml::from_str(manifest_source)
        .map_err(|error| format!("Cargo.toml is invalid while reading {name} dependency: {error}"))?;
    let dependency = manifest
        .get("dependencies")
        .and_then(|dependencies| dependencies.get(name))
        .ok_or_else(|| format!("Cargo.toml is missing {name} dependency"))?;
    let requirement = dependency
        .as_str()
        .or_else(|| dependency.get("version").and_then(toml::Value::as_str))
        .ok_or_else(|| format!("Cargo.toml {name} dependency has no string version requirement"))?;
    Ok(requirement.to_owned())
}

fn caret_lower_bound(requirement: &str, dependency: &str, source: &str) -> Result<semver::Version, String> {
    let parsed = semver::VersionReq::parse(requirement)
        .map_err(|error| format!("{source} {dependency} requirement {requirement:?} is invalid: {error}"))?;
    if parsed.comparators.len() != 1
        || parsed.comparators[0].op != semver::Op::Caret
        || parsed.comparators[0].minor.is_none()
        || !parsed.comparators[0].pre.is_empty()
    {
        return Err(format!(
            "{source} {dependency} requirement {requirement:?} is unsupported; expected one caret major.minor requirement"
        ));
    }
    let comparator = &parsed.comparators[0];
    Ok(semver::Version::new(
        comparator.major,
        comparator.minor.expect("minor was checked"),
        comparator.patch.unwrap_or(0),
    ))
}

fn validate_dependency_requirement(
    manifest_requirement: &str,
    document_dependency: &Dependency,
    dependency: &str,
) -> Result<(), String> {
    let manifest_lower = caret_lower_bound(manifest_requirement, dependency, "Cargo.toml")?;
    let document_lower = caret_lower_bound(&document_dependency.version, dependency, "documentation")?;
    let document_requirement = semver::VersionReq::parse(&document_dependency.version).map_err(|error| {
        format!(
            "documentation {dependency} requirement {:?} is invalid: {error}",
            document_dependency.version
        )
    })?;
    if document_lower < manifest_lower || !document_requirement.matches(&manifest_lower) {
        return Err(format!(
            "documentation {dependency} requirement {:?} is below or excludes Cargo.toml requirement {manifest_requirement:?}",
            document_dependency.version
        ));
    }
    Ok(())
}

fn package_version(root: &Path) -> String {
    let manifest = read_document(root, "Cargo.toml");
    manifest
        .lines()
        .find_map(|line| line.trim().strip_prefix("version = \""))
        .and_then(|line| line.strip_suffix('"'))
        .expect("package manifest should declare version")
        .to_owned()
}

fn manifest_dependency(name: &str, dependency: &Dependency) -> String {
    if dependency.features.is_empty() {
        format!("{name} = {:?}", dependency.version)
    } else {
        let features = dependency
            .features
            .iter()
            .map(|feature| format!("{feature:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "{name} = {{ version = {:?}, features = [{features}] }}",
            dependency.version
        )
    }
}

fn unique_programs(blocks: Vec<CodeBlock>) -> Vec<CodeBlock> {
    let mut unique = BTreeMap::new();
    for block in blocks {
        unique.entry(block.source.clone()).or_insert(block);
    }
    unique.into_values().collect()
}

fn cargo_consumer_manifest(
    package: &str,
    dependencies: &BTreeMap<String, Dependency>,
    crate_root: &Path,
    programs: &[CodeBlock],
) -> String {
    let mut manifest =
        format!("[package]\nname = {package:?}\nversion = \"0.0.0\"\nedition = \"2024\"\n\n[dependencies]\n");
    for (name, dependency) in dependencies {
        manifest.push_str(&manifest_dependency(name, dependency));
        manifest.push('\n');
    }
    manifest.push_str("\n[patch.crates-io]\nqubit-tokio-executor = { path = ");
    manifest.push_str(&format!("{:?}", crate_root.to_string_lossy()));
    manifest.push_str(" }\n");
    let executor_path = std::env::var_os("QUBIT_EXECUTOR_PATCH_PATH")
        .map(PathBuf::from)
        .or_else(|| {
            crate_root
                .parent()
                .map(|workspace| workspace.join("rs-executor"))
                .filter(|path| path.join("Cargo.toml").is_file())
        });
    if let Some(executor_path) = executor_path {
        manifest.push_str("qubit-executor = { path = ");
        manifest.push_str(&format!("{:?}", executor_path.to_string_lossy()));
        manifest.push_str(" }\n");
    }
    for (index, _) in programs.iter().enumerate() {
        let name = format!("doc_example_{index}");
        manifest.push_str(&format!("\n[[bin]]\nname = {name:?}\npath = \"src/bin/{name}.rs\"\n"));
    }
    manifest
}

#[test]
fn test_documentation_programs_compile_and_run_in_a_standalone_consumer() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let version = package_version(&root);
    let manifest_source = read_document(&root, "Cargo.toml");
    let mut dependencies = None;
    let mut programs = Vec::new();
    for relative in DOCUMENTS {
        let document = read_document(&root, relative);
        let parsed_dependencies = validate_document_dependencies(&document, relative, &version, &manifest_source)
            .unwrap_or_else(|error| panic!("{error}"));
        if let Some(expected) = &dependencies {
            assert_eq!(expected, &parsed_dependencies, "{relative} install block differs");
        } else {
            dependencies = Some(parsed_dependencies);
        }
        programs.extend(extract_programs(&document, relative).unwrap_or_else(|error| panic!("{error}")));
    }
    let programs = unique_programs(programs);
    assert!(!programs.is_empty(), "documentation contains no complete Rust programs");

    let temporary = TempDir::new("consumer");
    let source_dir = temporary.0.join("src/bin");
    fs::create_dir_all(&source_dir).expect("consumer source directory should be created");
    let dependencies = dependencies.expect("all docs should declare dependencies");
    for (index, program) in programs.iter().enumerate() {
        let name = format!("doc_example_{index}");
        let path = source_dir.join(format!("{name}.rs"));
        fs::write(&path, &program.source).unwrap_or_else(|error| {
            panic!(
                "failed to write {}:{}-{}: {error}",
                program.document, program.start_line, program.end_line
            )
        });
    }
    let manifest = cargo_consumer_manifest("tokio-executor-doc-examples", &dependencies, &root, &programs);
    fs::write(temporary.0.join("Cargo.toml"), manifest).expect("consumer manifest should be written");
    let target = temporary.0.join("target");
    let locations = programs
        .iter()
        .map(|program| format!("{}:{}-{}", program.document, program.start_line, program.end_line))
        .collect::<Vec<_>>()
        .join(", ");
    let build_timeout = parse_build_timeout(std::env::var_os(BUILD_TIMEOUT_ENV)).unwrap_or_else(|error| {
        panic!(
            "consumer build timeout configuration failed; status=unavailable; stdout/stderr unavailable because build did not start; document sources: {locations}; {error}"
        )
    });
    let mut build_command = Command::new("cargo");
    build_command
        .arg("build")
        .arg("--bins")
        .arg("--manifest-path")
        .arg(temporary.0.join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", &target)
        .current_dir(&temporary.0);
    let build = run_with_watchdog(build_command, build_timeout, "consumer build")
        .unwrap_or_else(|error| {
            panic!(
                "consumer build failed after {build_timeout:?}; status/stdout/stderr unavailable where build did not start; document sources: {locations}; {error}"
            )
        });
    assert!(
        !build.timed_out && build.status.is_some_and(|status| status.success()),
        "consumer build {} after {build_timeout:?}; status={:?}; stdout:\n{}\nstderr:\n{}\ndocument sources: {locations}",
        if build.timed_out { "timed out" } else { "failed" },
        build.status.and_then(|status| status.code()),
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr),
    );
    for (index, program) in programs.iter().enumerate() {
        let name = format!("doc_example_{index}");
        let executable = target
            .join("debug")
            .join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        let result = run_with_watchdog(Command::new(executable), RUN_TIMEOUT, &name)
            .unwrap_or_else(|error| panic!("{}:{} ({name}): {error}", program.document, program.start_line));
        assert!(
            !result.timed_out && result.status.is_some_and(|status| status.success()),
            "{}",
            output_diagnostic(program, &name, &result, RUN_TIMEOUT)
        );
    }
}

#[test]
fn test_document_example_watchdog_child() {
    let Ok(mode) = std::env::var(CHILD_MODE) else {
        return;
    };
    let ready = PathBuf::from(std::env::var_os("QUBIT_DOC_EXAMPLE_WATCHDOG_READY").expect("ready marker path"));
    fs::write(&ready, "ready").expect("child should signal startup");
    match mode.as_str() {
        "success" => println!("child success"),
        "failure" => {
            eprintln!("child failure output");
            std::process::exit(17);
        }
        "hang" => loop {
            thread::park_timeout(Duration::from_secs(1));
        },
        "descendant" => {
            let pid_file = PathBuf::from(std::env::var_os("QUBIT_DOC_EXAMPLE_WATCHDOG_PID").expect("pid file path"));
            let temporary_pid_file = pid_file.with_extension("pid.tmp");
            fs::write(&temporary_pid_file, std::process::id().to_string()).expect("descendant should write its pid");
            fs::rename(&temporary_pid_file, &pid_file).expect("descendant pid should be published atomically");
            loop {
                thread::park_timeout(Duration::from_secs(1));
            }
        }
        #[cfg(unix)]
        "root-with-descendant" | "root-exits-with-descendant" => {
            let pid_file = PathBuf::from(std::env::var_os("QUBIT_DOC_EXAMPLE_WATCHDOG_PID").expect("pid file path"));
            let executable = std::env::current_exe().expect("test executable path");
            let status = Command::new("sh")
                .args([
                    "-c",
                    "\"$1\" --exact test_document_example_watchdog_child --nocapture & while [ ! -s \"$2\" ]; do sleep 0.01; done",
                    "watchdog-launcher",
                ])
                .arg(executable)
                .arg(&pid_file)
                .env(CHILD_MODE, "descendant")
                .env("QUBIT_DOC_EXAMPLE_WATCHDOG_READY", &ready)
                .env("QUBIT_DOC_EXAMPLE_WATCHDOG_PID", &pid_file)
                .status()
                .expect("descendant launcher should start");
            assert!(
                status.success(),
                "descendant launcher should exit successfully: {status}"
            );
            fs::write(&ready, "root-ready").expect("root should signal descendant startup");
            println!("root output before watchdog completion");
            if mode == "root-exits-with-descendant" {
                return;
            }
            loop {
                thread::park_timeout(Duration::from_secs(1));
            }
        }
        #[cfg(not(unix))]
        "root-with-descendant" | "root-exits-with-descendant" => {
            panic!("descendant process-tree harness is only run on Unix")
        }
        "large-output" => {
            print!("{}", "o".repeat(256 * 1024));
            eprint!("{}", "e".repeat(256 * 1024));
        }
        other => panic!("unknown child mode: {other}"),
    }
}

#[test]
fn test_document_example_watchdog_handles_exit_output_and_hang() {
    let program = CodeBlock {
        document: "watchdog-test.md".to_owned(),
        start_line: 12,
        end_line: 14,
        source: String::new(),
    };
    for (mode, expected_code) in [("success", Some(0)), ("failure", Some(17)), ("large-output", Some(0))] {
        let ready_dir = TempDir::new("watchdog-ready");
        let ready = ready_dir.0.join("ready");
        let mut command = Command::new(std::env::current_exe().expect("test executable path"));
        command
            .args(["--exact", "test_document_example_watchdog_child", "--nocapture"])
            .env(CHILD_MODE, mode)
            .env("QUBIT_DOC_EXAMPLE_WATCHDOG_READY", &ready);
        let result = run_with_watchdog(command, Duration::from_secs(5), mode).expect("child should run");
        assert!(
            !result.timed_out,
            "{}",
            output_diagnostic(&program, mode, &result, Duration::from_secs(5))
        );
        assert_eq!(result.status.and_then(|status| status.code()), expected_code);
        assert!(ready.exists(), "child startup handshake should be present");
        if mode == "success" {
            assert!(String::from_utf8_lossy(&result.stdout).contains("child success"));
        }
        if mode == "failure" {
            assert!(String::from_utf8_lossy(&result.stderr).contains("child failure output"));
        }
        if mode == "large-output" {
            assert!(result.stdout.len() > 128 * 1024 && result.stderr.len() > 128 * 1024);
        }
    }

    let ready_dir = TempDir::new("watchdog-hang-ready");
    let ready = ready_dir.0.join("ready");
    let mut command = Command::new(std::env::current_exe().expect("test executable path"));
    command
        .args(["--exact", "test_document_example_watchdog_child", "--nocapture"])
        .env(CHILD_MODE, "hang")
        .env("QUBIT_DOC_EXAMPLE_WATCHDOG_READY", &ready);
    let worker = thread::spawn(move || run_with_watchdog(command, Duration::from_secs(2), "hang"));
    let handshake_deadline = Instant::now() + Duration::from_secs(5);
    while !ready.exists() && Instant::now() < handshake_deadline {
        thread::sleep(POLL_INTERVAL);
    }
    assert!(ready.exists(), "hanging child should reach startup handshake");
    let result = worker
        .join()
        .expect("watchdog worker should finish")
        .expect("child should launch");
    assert!(
        result.timed_out,
        "{}",
        output_diagnostic(&program, "hang", &result, Duration::from_secs(2))
    );
    assert!(result.status.is_some(), "timed-out child should be waited/reaped");
}

#[cfg(unix)]
#[test]
fn test_document_example_watchdog_cleans_descendants_when_root_hangs_or_exits() {
    fn is_running(pid: u32) -> bool {
        let output = Command::new("ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output();
        match output {
            Ok(output) if output.status.success() => {
                let state = String::from_utf8_lossy(&output.stdout);
                let Some(state) = state.split_whitespace().next() else {
                    return false;
                };
                !state.starts_with('Z') && !state.starts_with('X')
            }
            Ok(_) => false,
            Err(_) => true,
        }
    }

    for mode in ["root-with-descendant", "root-exits-with-descendant"] {
        let directory = TempDir::new("watchdog-tree");
        let ready = directory.0.join("ready");
        let pid_file = directory.0.join("descendant-pid");
        let command = {
            let mut command = Command::new(std::env::current_exe().expect("test executable path"));
            command
                .args(["--exact", "test_document_example_watchdog_child", "--nocapture"])
                .env(CHILD_MODE, mode)
                .env("QUBIT_DOC_EXAMPLE_WATCHDOG_READY", &ready)
                .env("QUBIT_DOC_EXAMPLE_WATCHDOG_PID", &pid_file);
            command
        };
        let (sender, receiver) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let _ = sender.send(run_with_watchdog(command, Duration::from_secs(1), mode));
        });
        let handshake_deadline = Instant::now() + Duration::from_secs(5);
        while (!ready.exists() || !pid_file.exists()) && Instant::now() < handshake_deadline {
            thread::sleep(POLL_INTERVAL);
        }
        assert!(
            ready.exists() && pid_file.exists(),
            "root/descendant handshake failed for {mode}"
        );
        let descendant: u32 = fs::read_to_string(&pid_file).unwrap().parse().unwrap();
        let result = receiver
            .recv_timeout(Duration::from_secs(8))
            .expect("watchdog must finish within external bound")
            .expect("watchdog should supervise tree");
        if mode == "root-with-descendant" {
            assert!(result.timed_out, "live root should time out");
        } else {
            assert!(!result.timed_out, "already-exited root should be a normal exit");
        }
        assert!(result.status.is_some(), "watchdog should reap root");
        assert!(
            String::from_utf8_lossy(&result.stdout).contains("root output before watchdog completion"),
            "watchdog should preserve root output"
        );
        let stop_deadline = Instant::now() + Duration::from_secs(3);
        while is_running(descendant) && Instant::now() < stop_deadline {
            thread::sleep(POLL_INTERVAL);
        }
        assert!(
            !is_running(descendant),
            "descendant {descendant} kept running after {mode}"
        );
    }
}

#[test]
fn test_install_parser_accepts_valid_toml_syntax() {
    let source = r#"
        [dependencies]
        qubit-tokio-executor = '0.12' # valid comment
        qubit-executor = "0.9\u002e1"
        tokio = {
            version = '1.54',
            features = [
                'macros',
                'rt-multi-thread',
                'time',
            ],
        }
    "#;
    let parsed = parse_install_toml(source).expect("valid TOML dependency syntax should parse");
    assert_eq!(parsed["qubit-tokio-executor"].version, "0.12");
    assert_eq!(parsed["qubit-executor"].version, "0.9.1");
    assert_eq!(
        parsed["tokio"],
        Dependency {
            version: "1.54".to_owned(),
            features: vec!["macros".to_owned(), "rt-multi-thread".to_owned(), "time".to_owned()],
        }
    );
}

#[test]
fn test_install_parser_rejects_invalid_toml_and_missing_required_features() {
    let invalid = [
        ("missing dependencies", "", None),
        ("empty dependencies", "[dependencies]", None),
        (
            "duplicate dependency key",
            "[dependencies]\ntokio = \"1\"\ntokio = \"1\"",
            None,
        ),
        (
            "duplicate dependencies table",
            "[dependencies]\ntokio = \"1\"\n[dependencies]\nqubit-executor = \"0.9.1\"",
            None,
        ),
        ("invalid TOML syntax", "[dependencies]\ntokio { version = \"1\" }", None),
        (
            "extra root table",
            "[dependencies]\ntokio = \"1\"\n[features]\ndefault = []",
            None,
        ),
        (
            "non-string shorthand version",
            "[dependencies]\ntokio = 1",
            Some(("tokio", "version")),
        ),
        (
            "non-string table version",
            "[dependencies]\ntokio = { version = 1 }",
            Some(("tokio", "version")),
        ),
        (
            "missing table version",
            "[dependencies]\ntokio = { features = [] }",
            Some(("tokio", "version")),
        ),
        (
            "empty version",
            "[dependencies]\ntokio = { version = \"\" }",
            Some(("tokio", "version")),
        ),
        (
            "non-array features",
            "[dependencies]\ntokio = { version = \"1\", features = \"time\" }",
            Some(("tokio", "features")),
        ),
        (
            "non-string feature",
            "[dependencies]\ntokio = { version = \"1\", features = [1] }",
            Some(("tokio", "features")),
        ),
        (
            "empty feature",
            "[dependencies]\ntokio = { version = \"1\", features = [\"\"] }",
            Some(("tokio", "features")),
        ),
        (
            "unknown dependency field",
            "[dependencies]\ntokio = { version = \"1\", default-features = false }",
            Some(("tokio", "default-features")),
        ),
        (
            "non-string dependency value",
            "[dependencies]\ntokio = true",
            Some(("tokio", "dependency")),
        ),
    ];
    for (case, source, context) in invalid {
        let error = parse_install_toml(source).expect_err(case);
        if let Some((dependency, field)) = context {
            assert!(error.contains(dependency), "{case}: {error}");
            assert!(error.contains(field), "{case}: {error}");
        }
    }
    let missing_feature = "```toml\n[dependencies]\nqubit-tokio-executor = \"0.12\"\nqubit-executor = \"0.9.1\"\ntokio = { version = \"1.54\", features = [\"macros\", \"time\"] }\n```";
    let manifest =
        "[dependencies]\nqubit-executor = \"^0.9.1\"\ntokio = { version = \"^1.54\", features = [\"rt\", \"sync\"] }";
    let error = validate_document_dependencies(missing_feature, "missing-feature.md", "0.12.0", manifest)
        .expect_err("missing Tokio feature should be rejected");
    assert!(error.contains("missing `rt-multi-thread` feature"), "{error}");
}

#[test]
fn test_build_timeout_parsing() {
    assert_eq!(parse_build_timeout(None).unwrap(), Duration::from_secs(600));
    assert_eq!(parse_build_timeout(Some("1".into())).unwrap(), Duration::from_secs(1));
    assert_eq!(
        parse_build_timeout(Some("600".into())).unwrap(),
        Duration::from_secs(600)
    );
    for invalid in ["0", "-1", "abc", "18446744073709551616", "18446744073709551615"] {
        assert!(
            parse_build_timeout(Some(invalid.into())).is_err(),
            "accepted {invalid:?}"
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        assert!(parse_build_timeout(Some(std::ffi::OsString::from_vec(vec![0xff]))).is_err());
    }
}

#[test]
fn test_manifest_dependency_requirement_reads_string_and_table_versions() {
    let manifest =
        "[dependencies]\ntokio = \"^1.54\"\nqubit-executor = { version = \"^0.9.1\", default-features = false }";
    assert_eq!(
        manifest_dependency_requirement(manifest, "tokio").expect("Tokio requirement should parse"),
        "^1.54"
    );
    assert_eq!(
        manifest_dependency_requirement(manifest, "qubit-executor").expect("qubit-executor requirement should parse"),
        "^0.9.1"
    );
    assert!(manifest_dependency_requirement(manifest, "missing").is_err());
    assert!(manifest_dependency_requirement("[dependencies", "tokio").is_err());
}

#[test]
fn test_document_dependency_requirements_follow_manifest_lower_bounds() {
    let dependency = |version: &str| Dependency {
        version: version.to_owned(),
        features: Vec::new(),
    };
    let manifest_154 = "[dependencies]\ntokio = { version = \"^1.54\" }";
    let manifest_154_requirement = manifest_dependency_requirement(manifest_154, "tokio").unwrap();
    assert!(validate_dependency_requirement(&manifest_154_requirement, &dependency("1.54"), "tokio").is_ok());
    assert!(validate_dependency_requirement(&manifest_154_requirement, &dependency("^1.54"), "tokio").is_ok());
    assert!(validate_dependency_requirement(&manifest_154_requirement, &dependency("1.53"), "tokio").is_err());
    let manifest_155 = "[dependencies]\ntokio = { version = \"^1.55\" }";
    let manifest_155_requirement = manifest_dependency_requirement(manifest_155, "tokio").unwrap();
    assert!(validate_dependency_requirement(&manifest_155_requirement, &dependency("1.54"), "tokio").is_err());
    assert!(validate_dependency_requirement("^0.9.1", &dependency("0.9"), "qubit-executor").is_err());
    assert!(validate_dependency_requirement("^1.54", &dependency("not-semver"), "tokio").is_err());
    assert!(caret_lower_bound(">=1.54, <2", "tokio", "Cargo.toml").is_err());
}

#[test]
fn test_extractor_rejects_documents_without_complete_programs() {
    assert!(extract_programs("# guide\n\n```bash\necho no\n```", "empty.md").is_err());
    assert!(extract_programs("```rust,fragment\nlet value = 42;\n```", "fragment-only.md").is_err());
    let mixed = "```rust,fragment\nlet value = 42;\n```\n```rust\nfn main() {}\n```";
    assert_eq!(extract_programs(mixed, "mixed.md").unwrap().len(), 1);
}

#[test]
fn test_extracted_compile_error_program_is_reported_with_its_source_location() {
    let source = "```rust\nfn main() { let _: u8 = \"not a number\"; }\n```";
    let program = extract_programs(source, "compile-error.md").expect("Rust block is extracted");
    let program = &program[0];
    let temporary = TempDir::new("compile-error");
    let rust_source = temporary.0.join("bad.rs");
    fs::write(&rust_source, &program.source).expect("negative example should be written");
    let output = Command::new("rustc")
        .arg("--edition=2024")
        .arg("--crate-name=bad_document_example")
        .arg("--emit=metadata")
        .arg(&rust_source)
        .current_dir(&temporary.0)
        .output()
        .expect("rustc should be available for the negative case");
    assert!(!output.status.success(), "compile-error block unexpectedly compiled");
    assert_eq!(program.document, "compile-error.md");
    assert_eq!(program.start_line, 2);
    assert_eq!(program.end_line, 3);
}
