//! Detached watcher lifecycle. Only the daemon owns daemon.lock; commands serialize
//! through lifecycle.lock and address the daemon through its private Unix socket.

use crate::{daemon, diagnostics, index};
use anyhow::{Context, Result, bail};
use clap::Subcommand;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(50);
/// Room for a row order of about ten thousand panes.
const REQUEST_LIMIT: u64 = 64 * 1024;
static SIGNAL: AtomicI32 = AtomicI32::new(0);

#[derive(Clone, Copy, Subcommand)]
pub enum Action {
    /// Start the watcher detached; succeeds if it is already running
    Start,
    /// Gracefully stop the managed watcher and start the current binary
    Restart,
    /// Gracefully stop the managed watcher
    Stop,
    #[command(hide = true)]
    Serve,
}

pub fn run(action: Action) -> Result<()> {
    match action {
        Action::Serve => serve(),
        _ => control(action).map_err(log_error),
    }
}

fn log_error(error: anyhow::Error) -> anyhow::Error {
    let message = format!("{error:#}");
    match diagnostics::record("lifecycle", &message).and_then(|()| diagnostics::log_path()) {
        Ok(path) => error.context(format!("daemon diagnostics: {}", path.display())),
        Err(log_error) => error.context(format!(
            "also failed to write daemon diagnostics: {log_error:#}"
        )),
    }
}

fn fatal_error(error: anyhow::Error) -> anyhow::Error {
    match daemon::report_fatal(&error).and_then(|()| diagnostics::log_path()) {
        Ok(path) => error.context(format!("daemon stopped; see {}", path.display())),
        Err(log_error) => error.context(format!(
            "daemon stopped; could not persist reason: {log_error:#}"
        )),
    }
}

fn open_lock(name: &str) -> Result<File> {
    let dir = index::state_dir()?;
    std::fs::create_dir_all(&dir)?;
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join(name))
        .with_context(|| format!("opening {name}"))
}

fn flock(file: &File, flags: i32) -> Result<bool> {
    if unsafe { libc::flock(file.as_raw_fd(), flags) } == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    if flags & libc::LOCK_NB != 0 && error.kind() == std::io::ErrorKind::WouldBlock {
        return Ok(false);
    }
    Err(error).context("locking daemon lifecycle")
}

fn lock_owned(name: &str) -> Result<bool> {
    let lock = open_lock(name)?;
    Ok(!flock(&lock, libc::LOCK_EX | libc::LOCK_NB)?)
}

fn socket_path() -> Result<PathBuf> {
    Ok(index::state_dir()?.join("daemon.sock"))
}

fn request(path: &Path, command: &str) -> Result<()> {
    let mut socket = UnixStream::connect(path).context("connecting to the managed daemon")?;
    socket.set_read_timeout(Some(TIMEOUT))?;
    socket.set_write_timeout(Some(TIMEOUT))?;
    socket.write_all(command.as_bytes())?;
    socket.shutdown(std::net::Shutdown::Write)?;
    let mut response = String::new();
    socket.read_to_string(&mut response)?;
    if response != "ok" {
        bail!("daemon control failed: {response}");
    }
    Ok(())
}

/// Hand the daemon the row order chosen by a sidebar's `s`. Once this returns, any tick stamped
/// later publishes it.
pub fn set_order(order: &[String]) -> Result<()> {
    request(&socket_path()?, &format!("order {}", order.join(" ")))
}

/// Queue a manual background organization job with the configuration the user confirmed.
pub fn organize(settings: &crate::autoname_config::Config) -> Result<()> {
    request(
        &socket_path()?,
        &format!("organize {}", serde_json::to_string(settings)?),
    )
}

fn control(action: Action) -> Result<()> {
    let lock = open_lock("lifecycle.lock")?;
    flock(&lock, libc::LOCK_EX)?;
    let socket = socket_path()?;
    if matches!(action, Action::Stop | Action::Restart) {
        stop(&socket)?;
    }
    if matches!(action, Action::Start | Action::Restart) {
        start(&socket)?;
    }
    Ok(())
}

fn stop(socket: &Path) -> Result<()> {
    if !lock_owned("daemon-control.lock")? {
        if lock_owned("daemon.lock")? {
            bail!("cannot stop daemon safely: an unmanaged watcher owns daemon.lock");
        }
        if socket.exists() {
            std::fs::remove_file(socket).context("removing stale daemon control socket")?;
        }
        return Ok(());
    }
    request(socket, "stop").context("cannot stop daemon safely through its control socket")?;
    let deadline = Instant::now() + TIMEOUT;
    while lock_owned("daemon-control.lock")? {
        if Instant::now() >= deadline {
            bail!("daemon did not stop within {} seconds", TIMEOUT.as_secs());
        }
        std::thread::sleep(POLL);
    }
    Ok(())
}

fn start(socket: &Path) -> Result<()> {
    let deadline = Instant::now() + TIMEOUT;
    while lock_owned("daemon-control.lock")? {
        if lock_owned("daemon.lock")? {
            return request(socket, "ping");
        }
        // The daemon keeps its control lock through setup and socket cleanup,
        // including the interval after daemon::run releases the index lock.
        if Instant::now() >= deadline {
            bail!(
                "daemon control did not become ready within {} seconds",
                TIMEOUT.as_secs()
            );
        }
        std::thread::sleep(POLL);
    }
    if lock_owned("daemon.lock")? {
        bail!("an unmanaged daemon owns daemon.lock; cannot start another watcher");
    }
    // A dead process leaves its socket behind. The command lock prevents another
    // start/stop from racing this cleanup, and both daemon locks are now free.
    if socket.exists() {
        std::fs::remove_file(socket).context("removing stale daemon control socket")?;
    }
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(diagnostics::log_path()?)
        .context("opening daemon output log")?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args(["daemon", "serve"])
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    // setsid detaches from the tmux job's session; all three standard streams are
    // replaced so tmux's job pipe closes as soon as this short start command exits.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let launched_at = index::now_ms();
    let mut child = command.spawn().context("launching detached daemon")?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Some(status) = child.try_wait()? {
            bail!("detached daemon exited during startup: {status}");
        }
        if socket.exists()
            && lock_owned("daemon.lock")?
            && index::read_any()?.is_some_and(|index| index.updated_at >= launched_at)
        {
            request(socket, "ping")?;
            return Ok(());
        }
        if Instant::now() >= deadline {
            // Address only the child we just spawned, never a guessed process name.
            if unsafe { libc::kill(child.id() as i32, libc::SIGTERM) } != 0 {
                return Err(std::io::Error::last_os_error())
                    .context("startup timed out and could not signal the detached child");
            }
            bail!(
                "detached daemon did not become ready within {} seconds; SIGTERM sent to its child",
                TIMEOUT.as_secs()
            );
        }
        std::thread::sleep(POLL);
    }
}

extern "C" fn signal_handler(signal: libc::c_int) {
    SIGNAL.store(signal, Ordering::Relaxed);
    daemon::request_shutdown();
}

fn install_signals() -> Result<()> {
    for signal in [libc::SIGTERM, libc::SIGINT] {
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        action.sa_sigaction = signal_handler as *const () as usize;
        unsafe { libc::sigemptyset(&mut action.sa_mask) };
        if unsafe { libc::sigaction(signal, &action, std::ptr::null_mut()) } != 0 {
            return Err(std::io::Error::last_os_error())
                .context("installing daemon signal handler");
        }
    }
    Ok(())
}

fn serve() -> Result<()> {
    // Keep ownership through socket cleanup, beyond daemon::run's index lock.
    let control_lock = open_lock("daemon-control.lock").map_err(fatal_error)?;
    if !flock(&control_lock, libc::LOCK_EX | libc::LOCK_NB).map_err(fatal_error)? {
        diagnostics::record(
            "lifecycle",
            "worker skipped: daemon-control.lock is already owned",
        )
        .map_err(log_error)?;
        return Ok(());
    }
    let (path, listener) = (|| -> Result<_> {
        install_signals()?;
        let path = socket_path()?;
        let listener = UnixListener::bind(&path).context("binding daemon control socket")?;
        let configure = (|| -> Result<()> {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
            listener.set_nonblocking(true)?;
            diagnostics::record(
                "lifecycle",
                &format!("daemon started (pid {})", std::process::id()),
            )
        })();
        if let Err(error) = configure {
            return match std::fs::remove_file(&path) {
                Ok(()) => Err(error),
                Err(cleanup) => Err(error.context(format!(
                    "also failed to remove daemon control socket: {cleanup}"
                ))),
            };
        }
        Ok((path, listener))
    })()
    .map_err(fatal_error)?;
    let done = Arc::new(AtomicBool::new(false));
    let control_done = Arc::clone(&done);
    let control = std::thread::spawn(move || {
        let result = control_loop(listener, &control_done);
        if result.is_err() {
            // A broken control plane must not leave an unmanaged watcher running.
            daemon::request_shutdown();
        }
        result
    });
    let result = daemon::run();
    done.store(true, Ordering::Relaxed);
    let cleanup = (|| -> Result<()> {
        let control_result = control
            .join()
            .map_err(|_| anyhow::anyhow!("daemon control thread panicked"))?;
        std::fs::remove_file(&path).context("removing daemon control socket")?;
        control_result?;
        let signal = SIGNAL.load(Ordering::Relaxed);
        if signal != 0 {
            diagnostics::record(
                "lifecycle",
                &format!("daemon stopped after signal {signal}"),
            )?;
        } else if result.is_ok() {
            diagnostics::record("lifecycle", "daemon stopped")?;
        }
        Ok(())
    })()
    .map_err(fatal_error);
    cleanup?;
    // daemon::run records its fatal errors; preserve that error without logging it twice.
    result
}

fn control_loop(listener: UnixListener, done: &AtomicBool) -> Result<()> {
    while !done.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((mut socket, _)) => {
                socket.set_read_timeout(Some(Duration::from_secs(1)))?;
                socket.set_write_timeout(Some(Duration::from_secs(1)))?;
                let mut command = String::new();
                (&mut socket)
                    .take(REQUEST_LIMIT)
                    .read_to_string(&mut command)?;
                if let Some(payload) = command.strip_prefix("organize ") {
                    let result = serde_json::from_str(payload)
                        .context("parsing organization settings")
                        .and_then(crate::organize::request);
                    let response = match result {
                        Ok(()) => "ok".into(),
                        Err(error) => format!("{error:#}"),
                    };
                    socket.write_all(response.as_bytes())?;
                    continue;
                }
                match command.as_str() {
                    "ping" => {}
                    "stop" => {
                        diagnostics::record("lifecycle", "graceful stop requested")?;
                        daemon::request_shutdown();
                    }
                    request => match request.strip_prefix("order ") {
                        Some(order) => daemon::request_order(
                            order.split_whitespace().map(String::from).collect(),
                        ),
                        None => bail!("unknown daemon control request: {command:?}"),
                    },
                }
                socket.write_all(b"ok")?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(POLL);
            }
            Err(error) => return Err(error).context("accepting daemon control request"),
        }
    }
    Ok(())
}
