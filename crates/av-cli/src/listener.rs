/// Background listener management.
///
/// `av listen` is the peer-responder process that must be running for cross-machine
/// queries to work. This module handles:
///
///   - Locating / writing the PID file (`<data_dir>/av-listener.pid`)
///   - Checking whether the process recorded in the PID file is still alive
///   - Spawning a detached `av listen` background process when needed
///
/// The PID file lives in the anta-vista data directory so it is per-user and
/// persists across terminal sessions. The listener process inherits no terminal
/// (stdin/stdout/stderr are redirected to /dev/null) so it survives the parent
/// `av` process exiting.

use std::path::PathBuf;
use std::process::Command;

// ── PID file location ────────────────────────────────────────────────────────

pub fn pid_path() -> Option<PathBuf> {
    av_core::paths::data_dir().map(|d| d.join("av-listener.pid"))
}

// ── Is the recorded listener process still alive? ───────────────────────────

pub fn is_running() -> bool {
    let pid = match read_pid() {
        Some(p) => p,
        None => return false,
    };
    process_alive(pid)
}

fn read_pid() -> Option<u32> {
    let path = pid_path()?;
    let text = std::fs::read_to_string(&path).ok()?;
    text.trim().parse::<u32>().ok()
}

fn process_alive(pid: u32) -> bool {
    // On Linux/macOS: sending signal 0 to a PID checks existence without disturbing it.
    // Returns true if the process exists and we have permission to signal it.
    // SAFETY: kill(pid, 0) is a well-defined POSIX call; we only use it for detection.
    #[cfg(unix)]
    {
        let ret = unsafe { libc::kill(pid as libc::pid_t, 0) };
        ret == 0
    }
    #[cfg(not(unix))]
    {
        // On non-Unix: fall back to checking /proc (won't reach here in practice
        // since we target Linux, but keeps it compiling).
        std::path::Path::new(&format!("/proc/{pid}")).exists()
    }
}

// ── Write our own PID (called by `av listen` at startup) ────────────────────

pub fn write_pid(pid: u32) {
    if let Some(path) = pid_path() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, pid.to_string());
    }
}

pub fn clear_pid() {
    if let Some(path) = pid_path() {
        let _ = std::fs::remove_file(path);
    }
}

// ── Ensure a listener is running, spawning one if not ───────────────────────

/// Call this from startup when x0x is available and the current command needs
/// the network.  Returns `true` if a listener was already running or was
/// successfully spawned, `false` if we could not spawn one (non-fatal — the
/// command still proceeds, it just may not get peer responses).
pub fn ensure_running() -> bool {
    if is_running() {
        return true;
    }
    spawn_background_listener()
}

fn spawn_background_listener() -> bool {
    // Resolve the path to the current `av` binary so we can re-invoke ourselves.
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return false,
    };

    let dev_null = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/null");

    let null_file = match dev_null {
        Ok(f) => f,
        Err(_) => return false,
    };

    // Spawn `av listen` as a fully detached process:
    //   - stdin/stdout/stderr → /dev/null
    //   - process_group(0) → its own process group so it isn't killed when the
    //     parent terminal closes
    use std::os::unix::process::CommandExt;
    let result = unsafe {
        Command::new(&exe)
            .arg("listen")
            .stdin(null_file.try_clone().unwrap_or_else(|_| {
                std::fs::File::open("/dev/null").unwrap()
            }))
            .stdout(null_file.try_clone().unwrap_or_else(|_| {
                std::fs::File::open("/dev/null").unwrap()
            }))
            .stderr(null_file)
            // Put the child in its own process group so Ctrl-C in the
            // parent terminal doesn't kill it.
            .pre_exec(|| {
                libc::setsid();
                Ok(())
            })
            .spawn()
    };

    match result {
        Ok(child) => {
            let pid = child.id();
            // Detach — we must not wait() on it.
            std::mem::forget(child);
            write_pid(pid);
            // Give it a moment to subscribe before the parent publishes.
            std::thread::sleep(std::time::Duration::from_millis(300));
            tracing::info!(pid, "av listen started in background");
            true
        }
        Err(e) => {
            tracing::warn!("could not spawn background av listen: {e}");
            false
        }
    }
}
