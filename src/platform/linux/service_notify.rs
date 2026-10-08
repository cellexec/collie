//! Minimal systemd service notification (`sd_notify(3)`) without libsystemd.
//!
//! A server started by a service manager with `Type=notify` (or any unit with
//! `NotifyAccess=` enabled) finds the manager's datagram socket in
//! `$NOTIFY_SOCKET`. Herdr reports readiness there, and a server that hands its
//! panes to a replacement process tells the manager the replacement's PID, so
//! the unit keeps supervising the live server instead of treating the old
//! server's exit as the end of the service.

use std::ffi::{OsStr, OsString};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::linux::net::SocketAddrExt;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::{SocketAddr, UnixDatagram};
use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use super::super::SERVICE_NOTIFY_SOCKET_ENV_VAR;

/// Upper bound for one notification, including waiting for room in a full
/// manager queue. Every socket operation is nonblocking and shares this
/// deadline, so a stalled manager can never strand the server.
const NOTIFY_TIMEOUT: Duration = Duration::from_secs(1);

static NOTIFY_SOCKET: OnceLock<Option<OsString>> = OnceLock::new();

/// Takes `$NOTIFY_SOCKET` out of the server's environment and remembers it.
///
/// Only the server talks to the service manager. Removing the variable keeps
/// panes, plugins, hooks, and helper processes from inheriting it.
pub(crate) fn capture_service_notify_socket() {
    let socket = std::env::var_os(SERVICE_NOTIFY_SOCKET_ENV_VAR).filter(|value| !value.is_empty());
    std::env::remove_var(SERVICE_NOTIFY_SOCKET_ENV_VAR);
    let _ = NOTIFY_SOCKET.set(socket);
}

fn captured_socket() -> Option<&'static OsStr> {
    NOTIFY_SOCKET.get().and_then(|socket| socket.as_deref())
}

/// Hands the captured socket to a replacement server so it can report a later
/// handoff of its own.
pub(crate) fn pass_service_notify_socket(command: &mut std::process::Command) {
    if let Some(socket) = captured_socket() {
        command.env(SERVICE_NOTIFY_SOCKET_ENV_VAR, socket);
    }
}

/// Reports that the server accepts connections. Failures are logged only.
pub(crate) fn notify_service_ready() {
    let Some(socket) = captured_socket() else {
        return;
    };
    match notify_ready_on(socket, Instant::now() + NOTIFY_TIMEOUT) {
        Ok(()) => tracing::info!("notified service manager that the server is ready"),
        Err(err) => tracing::warn!(err = %err, "failed to notify service manager of readiness"),
    }
}

/// Tells the service manager that `pid` is the service's main process now.
///
/// This must run in the current main process before it exits, so a unit with
/// `NotifyAccess=main` accepts it. Failures are logged and the caller keeps
/// shutting down; the whole exchange is bounded by one deadline.
pub(crate) fn notify_service_main_pid(pid: u32) {
    let Some(socket) = captured_socket() else {
        return;
    };
    match notify_main_pid_on(socket, pid, Instant::now() + NOTIFY_TIMEOUT) {
        Ok(()) => tracing::info!(pid, "notified service manager of the new main pid"),
        Err(err) => {
            tracing::warn!(pid, err = %err, "failed to notify service manager of the new main pid")
        }
    }
}

fn notify_ready_on(socket: &OsStr, deadline: Instant) -> io::Result<()> {
    let datagram = connect(socket)?;
    send_until(&datagram, b"READY=1", None, deadline)
}

fn notify_main_pid_on(socket: &OsStr, pid: u32, deadline: Instant) -> io::Result<()> {
    let datagram = connect(socket)?;
    let message = format!("MAINPID={pid}\nREADY=1");
    send_until(&datagram, message.as_bytes(), None, deadline)?;
    // The old server exits soon after this. Wait until the manager has handled
    // the message, so it already supervises the new PID when this one exits.
    wait_for_barrier(&datagram, deadline)
}

fn notify_socket_address(socket: &OsStr) -> io::Result<SocketAddr> {
    let bytes = socket.as_bytes();
    match bytes.first() {
        Some(b'/') => SocketAddr::from_pathname(Path::new(socket)),
        Some(b'@') => SocketAddr::from_abstract_name(&bytes[1..]),
        _ => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            format!("unsupported {SERVICE_NOTIFY_SOCKET_ENV_VAR} address"),
        )),
    }
}

fn connect(socket: &OsStr) -> io::Result<UnixDatagram> {
    let address = notify_socket_address(socket)?;
    let datagram = UnixDatagram::unbound()?;
    datagram.set_nonblocking(true)?;
    datagram.connect_addr(&address)?;
    Ok(datagram)
}

/// `sd_notify_barrier(3)`: send `BARRIER=1` with the write end of a pipe and
/// wait for the manager to close it, which it does after processing every
/// earlier message from this process.
fn wait_for_barrier(datagram: &UnixDatagram, deadline: Instant) -> io::Result<()> {
    let mut pipe = [0; 2];
    if unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let read_end = unsafe { OwnedFd::from_raw_fd(pipe[0]) };
    let write_end = unsafe { OwnedFd::from_raw_fd(pipe[1]) };
    send_until(
        datagram,
        b"BARRIER=1",
        Some(write_end.as_raw_fd()),
        deadline,
    )?;
    drop(write_end);
    // POLLHUP is always reported, so no requested events are needed.
    wait_for_fd(read_end.as_raw_fd(), 0, deadline).map_err(|err| {
        if err.kind() == io::ErrorKind::TimedOut {
            io::Error::new(
                io::ErrorKind::TimedOut,
                "service manager did not confirm the notification barrier",
            )
        } else {
            err
        }
    })
}

/// Sends one datagram on a nonblocking socket, waiting for queue space until
/// `deadline`.
fn send_until(
    datagram: &UnixDatagram,
    payload: &[u8],
    fd: Option<libc::c_int>,
    deadline: Instant,
) -> io::Result<()> {
    retry_send_until(
        payload.len(),
        deadline,
        Instant::now,
        || match fd {
            Some(fd) => send_with_fd(datagram, payload, fd),
            None => datagram.send(payload),
        },
        || wait_for_fd(datagram.as_raw_fd(), libc::POLLOUT, deadline),
    )
}

/// The first send always runs; every retry, whatever made it necessary, first
/// checks `deadline`, so readiness that keeps being lost cannot extend it.
fn retry_send_until(
    expected: usize,
    deadline: Instant,
    now: impl Fn() -> Instant,
    mut send_once: impl FnMut() -> io::Result<usize>,
    mut wait_writable: impl FnMut() -> io::Result<()>,
) -> io::Result<()> {
    let mut first_attempt = true;
    loop {
        if !first_attempt && now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "service manager notification did not fit before the deadline",
            ));
        }
        first_attempt = false;
        match send_once() {
            Ok(sent) if sent == expected => return Ok(()),
            Ok(_) => return Err(io::Error::other("short service notification write")),
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => {
                wait_writable().map_err(|err| {
                    if err.kind() == io::ErrorKind::TimedOut {
                        io::Error::new(
                            io::ErrorKind::TimedOut,
                            "service manager notification queue stayed full",
                        )
                    } else {
                        err
                    }
                })?;
            }
            Err(err) => return Err(err),
        }
    }
}

fn wait_for_fd(fd: libc::c_int, events: libc::c_short, deadline: Instant) -> io::Result<()> {
    wait_until(deadline, Instant::now, |timeout_ms| {
        let mut poll_fd = libc::pollfd {
            fd,
            events,
            revents: 0,
        };
        match unsafe { libc::poll(&mut poll_fd, 1, timeout_ms) } {
            ready if ready < 0 => Err(io::Error::last_os_error()),
            0 => Ok(false),
            _ => Ok(true),
        }
    })
}

/// Repeats `poll_once` until it reports readiness or `deadline` passes. Each
/// attempt gets only the time left, so interruptions never extend the wait.
fn wait_until(
    deadline: Instant,
    now: impl Fn() -> Instant,
    mut poll_once: impl FnMut(libc::c_int) -> io::Result<bool>,
) -> io::Result<()> {
    loop {
        let remaining = deadline.saturating_duration_since(now());
        let timeout_ms = remaining_poll_timeout_ms(remaining);
        match poll_once(timeout_ms) {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
        if timeout_ms == 0 || now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "timed out waiting for the service manager",
            ));
        }
    }
}

fn remaining_poll_timeout_ms(remaining: Duration) -> libc::c_int {
    // Round up, so a sub-millisecond remainder still sleeps instead of spinning.
    let millis = remaining.as_nanos().div_ceil(1_000_000);
    millis.min(libc::c_int::MAX as u128) as libc::c_int
}

fn send_with_fd(datagram: &UnixDatagram, payload: &[u8], fd: libc::c_int) -> io::Result<usize> {
    let iov = [libc::iovec {
        iov_base: payload.as_ptr() as *mut libc::c_void,
        iov_len: payload.len(),
    }];
    let fd_bytes = std::mem::size_of_val(&fd);
    let mut control = vec![0u8; unsafe { libc::CMSG_SPACE(fd_bytes as u32) as usize }];
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = iov.as_ptr() as *mut libc::iovec;
    msg.msg_iovlen = iov.len() as _;
    msg.msg_control = control.as_mut_ptr() as *mut libc::c_void;
    msg.msg_controllen = control.len() as _;

    unsafe {
        let cmsg = libc::CMSG_FIRSTHDR(&msg);
        if cmsg.is_null() {
            return Err(io::Error::other("failed to allocate fd control message"));
        }
        (*cmsg).cmsg_level = libc::SOL_SOCKET;
        (*cmsg).cmsg_type = libc::SCM_RIGHTS;
        (*cmsg).cmsg_len = libc::CMSG_LEN(fd_bytes as u32) as _;
        std::ptr::copy_nonoverlapping(
            (&fd as *const libc::c_int).cast::<u8>(),
            libc::CMSG_DATA(cmsg),
            fd_bytes,
        );
        let sent = libc::sendmsg(
            datagram.as_raw_fd(),
            &msg,
            libc::MSG_NOSIGNAL | libc::MSG_DONTWAIT,
        );
        if sent < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(sent as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receive(receiver: &UnixDatagram) -> String {
        let mut buffer = [0u8; 256];
        let len = receiver.recv(&mut buffer).expect("receive notification");
        String::from_utf8_lossy(&buffer[..len]).into_owned()
    }

    fn unique_name(label: &str) -> String {
        format!(
            "herdr-notify-test-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        )
    }

    fn abstract_receiver(label: &str) -> (UnixDatagram, String) {
        let name = unique_name(label);
        let address = SocketAddr::from_abstract_name(name.as_bytes()).unwrap();
        let receiver = UnixDatagram::bind_addr(&address).unwrap();
        (receiver, format!("@{name}"))
    }

    /// Fills the receiver's queue until a fresh sender cannot add a datagram.
    /// Several senders are needed because each one also has its own send
    /// buffer limit, which can run out before the receiver queue does.
    fn saturate(socket: &str) -> Vec<UnixDatagram> {
        let mut senders = Vec::new();
        for _ in 0..4096 {
            let sender = connect(OsStr::new(socket)).unwrap();
            match sender.send(b"x") {
                Ok(_) => {}
                Err(err) if err.kind() == io::ErrorKind::WouldBlock => return senders,
                Err(err) => panic!("saturate notify socket: {err}"),
            }
            while sender.send(b"x").is_ok() {}
            senders.push(sender);
        }
        panic!("notify socket queue never filled");
    }

    fn drain(receiver: &UnixDatagram) -> Vec<String> {
        receiver.set_nonblocking(true).unwrap();
        let mut messages = Vec::new();
        let mut buffer = [0u8; 256];
        while let Ok(len) = receiver.recv(&mut buffer) {
            messages.push(String::from_utf8_lossy(&buffer[..len]).into_owned());
        }
        messages
    }

    #[test]
    fn ready_reaches_a_path_notify_socket() {
        let dir = std::env::temp_dir().join(unique_name("path"));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("notify.sock");
        let receiver = UnixDatagram::bind(&path).unwrap();

        notify_ready_on(path.as_os_str(), Instant::now() + Duration::from_secs(5)).unwrap();

        assert_eq!(receive(&receiver), "READY=1");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn main_pid_reaches_an_abstract_notify_socket_and_waits_for_the_barrier() {
        let (receiver, socket) = abstract_receiver("abstract");
        let reader = std::thread::spawn(move || {
            let main_pid = receive(&receiver);
            // A plain receive discards the passed pipe end, which closes it the
            // same way the service manager does after processing the barrier.
            let barrier = receive(&receiver);
            (main_pid, barrier)
        });

        notify_main_pid_on(
            OsStr::new(&socket),
            4242,
            Instant::now() + Duration::from_secs(5),
        )
        .unwrap();

        let (main_pid, barrier) = reader.join().unwrap();
        assert_eq!(main_pid, "MAINPID=4242\nREADY=1");
        assert_eq!(barrier, "BARRIER=1");
    }

    #[test]
    fn barrier_times_out_when_the_manager_never_reads() {
        let (receiver, socket) = abstract_receiver("silent");

        let started = Instant::now();
        let err = notify_main_pid_on(
            OsStr::new(&socket),
            4242,
            Instant::now() + Duration::from_millis(50),
        )
        .unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(drain(&receiver), ["MAINPID=4242\nREADY=1", "BARRIER=1"]);
    }

    #[test]
    fn main_pid_gives_up_at_the_deadline_when_the_queue_is_full() {
        let (receiver, socket) = abstract_receiver("full-main-pid");
        let _senders = saturate(&socket);

        let started = Instant::now();
        let err = notify_main_pid_on(
            OsStr::new(&socket),
            4242,
            Instant::now() + Duration::from_millis(100),
        )
        .unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::TimedOut, "{err}");
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(drain(&receiver).iter().all(|message| message == "x"));
    }

    #[test]
    fn barrier_gives_up_at_the_deadline_when_the_queue_fills_after_main_pid() {
        let (receiver, socket) = abstract_receiver("full-barrier");
        let _senders = saturate(&socket);
        // Room for exactly one more datagram: MAINPID fits, BARRIER does not.
        assert_eq!(receive(&receiver), "x");

        let started = Instant::now();
        let err = notify_main_pid_on(
            OsStr::new(&socket),
            4242,
            Instant::now() + Duration::from_millis(100),
        )
        .unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::TimedOut, "{err}");
        assert!(started.elapsed() < Duration::from_secs(1));
        let messages = drain(&receiver);
        assert_eq!(
            messages.last().map(String::as_str),
            Some("MAINPID=4242\nREADY=1")
        );
        assert!(!messages.iter().any(|message| message == "BARRIER=1"));
    }

    #[test]
    fn ready_gives_up_at_the_deadline_when_the_queue_is_full() {
        let (_receiver, socket) = abstract_receiver("full-ready");
        let _senders = saturate(&socket);

        let started = Instant::now();
        let err = notify_ready_on(
            OsStr::new(&socket),
            Instant::now() + Duration::from_millis(100),
        )
        .unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::TimedOut, "{err}");
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    /// A fake monotonic clock, so deadline tests don't depend on the scheduler.
    struct FakeClock {
        start: Instant,
        elapsed: std::cell::Cell<Duration>,
    }

    impl FakeClock {
        fn new() -> Self {
            Self {
                start: Instant::now(),
                elapsed: std::cell::Cell::new(Duration::ZERO),
            }
        }

        fn now(&self) -> Instant {
            self.start + self.elapsed.get()
        }

        fn advance(&self, by: Duration) {
            self.elapsed.set(self.elapsed.get() + by);
        }

        fn after(&self, by: Duration) -> Instant {
            self.start + by
        }
    }

    #[test]
    fn interruptions_do_not_extend_the_deadline() {
        let clock = FakeClock::new();
        let mut timeouts = Vec::new();

        let err = wait_until(
            clock.after(Duration::from_millis(100)),
            || clock.now(),
            |timeout_ms| {
                timeouts.push(timeout_ms);
                clock.advance(Duration::from_millis(15));
                Err(io::Error::from(io::ErrorKind::Interrupted))
            },
        )
        .unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert_eq!(timeouts, [100, 85, 70, 55, 40, 25, 10]);
    }

    #[test]
    fn interrupted_sends_stop_at_the_deadline() {
        let clock = FakeClock::new();
        let mut attempts = 0;

        let err = retry_send_until(
            7,
            clock.after(Duration::from_millis(50)),
            || clock.now(),
            || {
                attempts += 1;
                clock.advance(Duration::from_millis(10));
                Err(io::Error::from(io::ErrorKind::Interrupted))
            },
            || panic!("an interrupted send must not wait for queue space"),
        )
        .unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert_eq!(attempts, 5);
    }

    #[test]
    fn lost_readiness_does_not_extend_the_deadline() {
        // poll keeps reporting room, but the queue fills again before each send.
        let clock = FakeClock::new();
        let mut sends = 0;
        let mut waits = 0;

        let err = retry_send_until(
            7,
            clock.after(Duration::from_millis(50)),
            || clock.now(),
            || {
                sends += 1;
                Err(io::Error::from(io::ErrorKind::WouldBlock))
            },
            || {
                waits += 1;
                clock.advance(Duration::from_millis(10));
                Ok(())
            },
        )
        .unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
        assert_eq!(sends, 5);
        assert_eq!(waits, 5);
    }

    #[test]
    fn the_first_send_runs_even_at_the_deadline() {
        let clock = FakeClock::new();
        let mut sends = 0;

        retry_send_until(
            7,
            clock.now(),
            || clock.now(),
            || {
                sends += 1;
                Ok(7)
            },
            || panic!("a successful send must not wait"),
        )
        .unwrap();

        assert_eq!(sends, 1);
    }

    #[test]
    fn an_interrupted_send_is_retried_before_the_deadline() {
        let mut attempts = 0;

        retry_send_until(
            7,
            Instant::now() + Duration::from_secs(5),
            Instant::now,
            || {
                attempts += 1;
                if attempts < 3 {
                    Err(io::Error::from(io::ErrorKind::Interrupted))
                } else {
                    Ok(7)
                }
            },
            || panic!("an interrupted send must not wait for queue space"),
        )
        .unwrap();

        assert_eq!(attempts, 3);
    }

    #[test]
    fn readiness_after_interruptions_succeeds() {
        let mut attempts = 0;

        wait_until(
            Instant::now() + Duration::from_secs(5),
            Instant::now,
            |_| {
                attempts += 1;
                if attempts < 4 {
                    Err(io::Error::from(io::ErrorKind::Interrupted))
                } else {
                    Ok(true)
                }
            },
        )
        .unwrap();

        assert_eq!(attempts, 4);
    }

    #[test]
    fn sub_millisecond_remainders_round_up() {
        assert_eq!(remaining_poll_timeout_ms(Duration::ZERO), 0);
        assert_eq!(remaining_poll_timeout_ms(Duration::from_micros(1)), 1);
        assert_eq!(remaining_poll_timeout_ms(Duration::from_millis(7)), 7);
    }

    #[test]
    fn relative_and_vsock_addresses_are_rejected() {
        for socket in ["notify.sock", "vsock:2:1234"] {
            let err = notify_ready_on(OsStr::new(socket), Instant::now()).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::Unsupported, "{socket}");
        }
    }
}
