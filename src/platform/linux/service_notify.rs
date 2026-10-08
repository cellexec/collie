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
use std::time::Duration;

use super::super::SERVICE_NOTIFY_SOCKET_ENV_VAR;

/// How long the old server waits for the manager to process the main PID
/// change before it continues shutting down.
const BARRIER_TIMEOUT: Duration = Duration::from_secs(1);

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

/// Reports that the server accepts connections.
pub(crate) fn notify_service_ready() {
    let Some(socket) = captured_socket() else {
        return;
    };
    match send_notification(socket, "READY=1") {
        Ok(()) => tracing::info!("notified service manager that the server is ready"),
        Err(err) => tracing::warn!(err = %err, "failed to notify service manager of readiness"),
    }
}

/// Tells the service manager that `pid` is the service's main process now.
///
/// This must run in the current main process before it exits, so a unit with
/// `NotifyAccess=main` accepts it.
pub(crate) fn notify_service_main_pid(pid: u32) {
    let Some(socket) = captured_socket() else {
        return;
    };
    match notify_main_pid_on(socket, pid, BARRIER_TIMEOUT) {
        Ok(()) => tracing::info!(pid, "notified service manager of the new main pid"),
        Err(err) => {
            tracing::warn!(pid, err = %err, "failed to notify service manager of the new main pid")
        }
    }
}

fn notify_main_pid_on(socket: &OsStr, pid: u32, barrier_timeout: Duration) -> io::Result<()> {
    send_notification(socket, &format!("MAINPID={pid}\nREADY=1"))?;
    // The old server exits soon after this. Wait until the manager has handled
    // the message, so it already supervises the new PID when this one exits.
    wait_for_barrier(socket, barrier_timeout)
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
    datagram.connect_addr(&address)?;
    Ok(datagram)
}

fn send_notification(socket: &OsStr, message: &str) -> io::Result<()> {
    let datagram = connect(socket)?;
    let sent = datagram.send(message.as_bytes())?;
    if sent != message.len() {
        return Err(io::Error::other("short service notification write"));
    }
    Ok(())
}

/// `sd_notify_barrier(3)`: send `BARRIER=1` with the write end of a pipe and
/// wait for the manager to close it, which it does after processing every
/// earlier message from this process.
fn wait_for_barrier(socket: &OsStr, timeout: Duration) -> io::Result<()> {
    let datagram = connect(socket)?;
    let mut pipe = [0; 2];
    if unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let read_end = unsafe { OwnedFd::from_raw_fd(pipe[0]) };
    let write_end = unsafe { OwnedFd::from_raw_fd(pipe[1]) };
    send_with_fd(&datagram, b"BARRIER=1", write_end.as_raw_fd())?;
    drop(write_end);

    let mut poll_fd = libc::pollfd {
        fd: read_end.as_raw_fd(),
        events: 0,
        revents: 0,
    };
    let timeout_ms = timeout.as_millis().min(i32::MAX as u128) as i32;
    loop {
        let ready = unsafe { libc::poll(&mut poll_fd, 1, timeout_ms) };
        if ready < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        if ready == 0 {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "service manager did not confirm the notification barrier",
            ));
        }
        return Ok(());
    }
}

fn send_with_fd(datagram: &UnixDatagram, payload: &[u8], fd: libc::c_int) -> io::Result<()> {
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
        if libc::sendmsg(datagram.as_raw_fd(), &msg, libc::MSG_NOSIGNAL) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
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

    #[test]
    fn ready_reaches_a_path_notify_socket() {
        let dir = std::env::temp_dir().join(unique_name("path"));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("notify.sock");
        let receiver = UnixDatagram::bind(&path).unwrap();

        send_notification(path.as_os_str(), "READY=1").unwrap();

        assert_eq!(receive(&receiver), "READY=1");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn main_pid_reaches_an_abstract_notify_socket_and_waits_for_the_barrier() {
        let name = unique_name("abstract");
        let address = SocketAddr::from_abstract_name(name.as_bytes()).unwrap();
        let receiver = UnixDatagram::bind_addr(&address).unwrap();
        let reader = std::thread::spawn(move || {
            let main_pid = receive(&receiver);
            // A plain receive discards the passed pipe end, which closes it the
            // same way the service manager does after processing the barrier.
            let barrier = receive(&receiver);
            (main_pid, barrier)
        });

        notify_main_pid_on(
            OsStr::new(&format!("@{name}")),
            4242,
            Duration::from_secs(5),
        )
        .unwrap();

        let (main_pid, barrier) = reader.join().unwrap();
        assert_eq!(main_pid, "MAINPID=4242\nREADY=1");
        assert_eq!(barrier, "BARRIER=1");
    }

    #[test]
    fn barrier_times_out_when_the_manager_never_reads() {
        let name = unique_name("silent");
        let address = SocketAddr::from_abstract_name(name.as_bytes()).unwrap();
        let _receiver = UnixDatagram::bind_addr(&address).unwrap();

        let err = wait_for_barrier(OsStr::new(&format!("@{name}")), Duration::from_millis(50))
            .unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    }

    #[test]
    fn relative_and_vsock_addresses_are_rejected() {
        for socket in ["notify.sock", "vsock:2:1234"] {
            let err = send_notification(OsStr::new(socket), "READY=1").unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::Unsupported, "{socket}");
        }
    }
}
