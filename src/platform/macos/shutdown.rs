//! Restart and shutdown detection through loginwindow's notify(3) keys.
//!
//! During a restart or shutdown, loginwindow kills LaunchServices-registered
//! background processes (for example Node-based agents that set
//! `process.title`) right after its point of no return, 15-20 seconds before
//! launchd sends SIGTERM to the server. The server must save its intact session
//! before it applies those exits.
//!
//! loginwindow posts these keys as system notifications. They are undocumented,
//! so a missing key degrades to the previous behavior (save on SIGTERM).

use std::ffi::{c_char, c_int, CString};
use std::io;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const LOGINWINDOW_PREFIX: &str = "com.apple.system.loginwindow.";
const NOTIFY_STATUS_OK: u32 = 0;
const NOTIFY_REUSE: c_int = 1;

unsafe extern "C" {
    fn notify_register_file_descriptor(
        name: *const c_char,
        notify_fd: *mut c_int,
        flags: c_int,
        out_token: *mut c_int,
    ) -> u32;
    fn notify_cancel(token: c_int) -> u32;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoginwindowEvent {
    /// A restart or shutdown started. The user or an app can still cancel it.
    ShutdownIntent,
    /// The pending logout, restart or shutdown was cancelled.
    Cancelled,
    /// loginwindow passed its point of no return and starts killing processes.
    NoReturn,
}

const LOGINWINDOW_KEYS: [(&str, LoginwindowEvent); 6] = [
    ("restartinitiated", LoginwindowEvent::ShutdownIntent),
    ("shutdownInitiated", LoginwindowEvent::ShutdownIntent),
    ("likelyShutdown", LoginwindowEvent::ShutdownIntent),
    ("logoutcancelled", LoginwindowEvent::Cancelled),
    ("logoutNoReturn", LoginwindowEvent::NoReturn),
    ("shutdownNoReturn", LoginwindowEvent::NoReturn),
];

/// A plain logout also reaches the point of no return, but the server is meant
/// to outlive it, so only a preceding restart or shutdown intent arms the exit.
#[derive(Debug, Default)]
struct ShutdownLatch {
    armed: bool,
}

impl ShutdownLatch {
    /// Returns true when the server should save and exit now.
    fn observe(&mut self, event: LoginwindowEvent) -> bool {
        match event {
            LoginwindowEvent::ShutdownIntent => {
                self.armed = true;
                false
            }
            LoginwindowEvent::Cancelled => {
                self.armed = false;
                false
            }
            LoginwindowEvent::NoReturn => self.armed,
        }
    }
}

pub(crate) fn monitor_host_shutdown(
    requested: Arc<AtomicBool>,
    wake: impl Fn() + Send + Sync + 'static,
) -> Option<tokio::task::JoinHandle<()>> {
    start_monitor(LOGINWINDOW_PREFIX, requested, wake)
}

fn start_monitor(
    prefix: &str,
    requested: Arc<AtomicBool>,
    wake: impl Fn() + Send + Sync + 'static,
) -> Option<tokio::task::JoinHandle<()>> {
    let registration = match Registration::new(prefix) {
        Ok(registration) => registration,
        Err(err) => {
            tracing::debug!(err = %err, "host shutdown notification unavailable");
            return None;
        }
    };
    let (stop_tx, stop_rx) = match UnixStream::pair() {
        Ok(pair) => pair,
        Err(err) => {
            tracing::debug!(err = %err, "host shutdown notification unavailable");
            return None;
        }
    };
    let spawned = std::thread::Builder::new()
        .name("herdr-host-shutdown".into())
        .spawn(move || watch(registration, stop_rx, &requested, &wake));
    if let Err(err) = spawned {
        tracing::debug!(err = %err, "host shutdown notification unavailable");
        return None;
    }
    tracing::debug!("host shutdown notification ready");
    // Dropping the monitor aborts this task, which closes `stop_tx` and ends the watcher.
    Some(tokio::spawn(async move {
        let _stop_tx = stop_tx;
        std::future::pending::<()>().await;
    }))
}

fn watch(
    registration: Registration,
    stop: UnixStream,
    requested: &AtomicBool,
    wake: &(impl Fn() + Send + Sync),
) {
    let mut latch = ShutdownLatch::default();
    let mut pending = Vec::with_capacity(8);
    let mut buf = [0u8; 64];
    loop {
        let mut fds = [
            libc::pollfd {
                fd: registration.fd,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: stop.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        // SAFETY: `fds` is a valid array of two pollfd entries for this call.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, -1) };
        if ready < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            tracing::debug!(err = %err, "host shutdown notification stopped");
            return;
        }
        if fds[1].revents != 0 {
            return;
        }
        if fds[0].revents == 0 {
            continue;
        }
        // SAFETY: `buf` is valid for its length and the descriptor stays open
        // while `registration` is alive.
        let read = unsafe { libc::read(registration.fd, buf.as_mut_ptr().cast(), buf.len()) };
        let read = match read {
            0 => return,
            read if read > 0 => read as usize,
            _ => {
                let err = io::Error::last_os_error();
                if err.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                tracing::debug!(err = %err, "host shutdown notification stopped");
                return;
            }
        };
        pending.extend_from_slice(&buf[..read]);
        let complete = pending.len() - pending.len() % 4;
        for bytes in pending[..complete].chunks_exact(4) {
            // notifyd writes each token in network byte order.
            let token = c_int::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            let Some(event) = registration.event(token) else {
                continue;
            };
            tracing::info!(?event, "macos loginwindow notification");
            if latch.observe(event) {
                tracing::info!(
                    "host shutdown requested; preserving session before pane termination"
                );
                requested.store(true, Ordering::Release);
                wake();
                return;
            }
        }
        pending.drain(..complete);
    }
}

/// notify(3) tokens sharing one descriptor. libnotify closes the descriptor
/// once the last token is cancelled.
struct Registration {
    fd: RawFd,
    tokens: Vec<(c_int, LoginwindowEvent)>,
}

impl Registration {
    fn new(prefix: &str) -> io::Result<Self> {
        let mut registration = Self {
            fd: -1,
            tokens: Vec::with_capacity(LOGINWINDOW_KEYS.len()),
        };
        for (key, event) in LOGINWINDOW_KEYS {
            let name = CString::new(format!("{prefix}{key}"))
                .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;
            let flags = if registration.tokens.is_empty() {
                0
            } else {
                NOTIFY_REUSE
            };
            let mut token = -1;
            // SAFETY: `name` is NUL-terminated and both out-pointers are valid.
            let status = unsafe {
                notify_register_file_descriptor(
                    name.as_ptr(),
                    &mut registration.fd,
                    flags,
                    &mut token,
                )
            };
            if status != NOTIFY_STATUS_OK {
                return Err(io::Error::other(format!(
                    "notify_register_file_descriptor({key}) failed with status {status}"
                )));
            }
            registration.tokens.push((token, event));
        }
        // SAFETY: the descriptor was created by libnotify and is still registered.
        unsafe {
            libc::fcntl(registration.fd, libc::F_SETFD, libc::FD_CLOEXEC);
        }
        Ok(registration)
    }

    fn event(&self, token: c_int) -> Option<LoginwindowEvent> {
        self.tokens
            .iter()
            .find(|(registered, _)| *registered == token)
            .map(|(_, event)| *event)
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        for (token, _) in &self.tokens {
            // SAFETY: each token came from a successful registration and is cancelled once.
            unsafe {
                notify_cancel(*token);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    use LoginwindowEvent::{Cancelled, NoReturn, ShutdownIntent};

    unsafe extern "C" {
        fn notify_post(name: *const c_char) -> u32;
    }

    #[test]
    fn point_of_no_return_requires_restart_or_shutdown_intent() {
        let mut latch = ShutdownLatch::default();
        assert!(!latch.observe(NoReturn), "plain logout keeps the server");

        let mut latch = ShutdownLatch::default();
        assert!(!latch.observe(ShutdownIntent));
        assert!(latch.observe(NoReturn));
    }

    #[test]
    fn cancelled_logout_disarms_until_the_next_intent() {
        let mut latch = ShutdownLatch::default();
        assert!(!latch.observe(ShutdownIntent));
        assert!(!latch.observe(Cancelled));
        assert!(!latch.observe(NoReturn));
        assert!(!latch.observe(ShutdownIntent));
        assert!(latch.observe(NoReturn));
    }

    #[test]
    fn registers_loginwindow_keys() {
        let key = |name| {
            LOGINWINDOW_KEYS
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, event)| *event)
        };
        assert_eq!(LOGINWINDOW_PREFIX, "com.apple.system.loginwindow.");
        for intent in ["restartinitiated", "shutdownInitiated", "likelyShutdown"] {
            assert_eq!(key(intent), Some(ShutdownIntent), "{intent}");
        }
        assert_eq!(key("logoutcancelled"), Some(Cancelled));
        assert_eq!(key("logoutNoReturn"), Some(NoReturn));
        assert_eq!(key("shutdownNoReturn"), Some(NoReturn));
        assert_eq!(key("logoutInitiated"), None, "plain logout must not arm");
    }

    // Unprivileged processes cannot post `com.apple.system.*`, so these tests
    // use a private prefix with the same suffixes and never touch the real keys.
    fn post(prefix: &str, key: &str) {
        let name = CString::new(format!("{prefix}{key}")).unwrap();
        assert_eq!(unsafe { notify_post(name.as_ptr()) }, NOTIFY_STATUS_OK);
        std::thread::sleep(Duration::from_millis(50));
    }

    fn wait_for(flag: &AtomicBool, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if flag.load(Ordering::Acquire) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        flag.load(Ordering::Acquire)
    }

    fn monitor(case: &str) -> (String, Arc<AtomicBool>, tokio::task::JoinHandle<()>) {
        let prefix = format!("dev.herdr.test.{}.{case}.", std::process::id());
        let requested = Arc::new(AtomicBool::new(false));
        let task = start_monitor(&prefix, requested.clone(), || {}).expect("notify registration");
        (prefix, requested, task)
    }

    #[tokio::test]
    async fn notify_restart_sequence_requests_host_shutdown() {
        let (prefix, requested, task) = monitor("restart");
        post(&prefix, "restartinitiated");
        post(&prefix, "likelyShutdown");
        assert!(!wait_for(&requested, Duration::from_millis(100)));
        post(&prefix, "logoutNoReturn");
        assert!(wait_for(&requested, Duration::from_secs(5)));
        task.abort();
    }

    #[tokio::test]
    async fn notify_plain_or_cancelled_logout_keeps_server() {
        let (prefix, requested, task) = monitor("logout");
        post(&prefix, "logoutInitiated");
        post(&prefix, "logoutNoReturn");
        post(&prefix, "shutdownInitiated");
        post(&prefix, "logoutcancelled");
        post(&prefix, "shutdownNoReturn");
        assert!(!wait_for(&requested, Duration::from_millis(300)));
        task.abort();
    }
}
