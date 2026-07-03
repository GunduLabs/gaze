use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};

use console::style;

const NOTIFY_FD: RawFd = 3;

const REGISTER_TIMEOUT_MS: libc::c_int = 5_000;

pub struct PolkitAgent {
    child: Option<Child>,
}

impl PolkitAgent {
    pub fn spawn() -> Self {
        match Self::try_spawn() {
            Ok(agent) => agent,
            Err(err) => {
                eprintln!(
                    "{} could not start pkttyagent ({err}); if authorization \
                     fails, run `gaze` from a graphical session or install \
                     polkit's tty agent.",
                    style("note:").yellow().bold()
                );
                PolkitAgent { child: None }
            }
        }
    }

    fn try_spawn() -> std::io::Result<Self> {
        let (read_fd, write_fd) = pipe_cloexec()?;
        let write_raw = write_fd.as_raw_fd();

        let mut cmd = Command::new("pkttyagent");
        cmd.arg("--fallback")
            .arg("--notify-fd")
            .arg(NOTIFY_FD.to_string())
            .arg("--process")
            .arg(std::process::id().to_string())
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());

        unsafe {
            cmd.pre_exec(move || {
                if libc::dup2(write_raw, NOTIFY_FD) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }

        let child = cmd.spawn()?;

        drop(write_fd);
        wait_for_registration(read_fd);

        Ok(PolkitAgent { child: Some(child) })
    }
}

impl Drop for PolkitAgent {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn pipe_cloexec() -> std::io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0 as RawFd; 2];
    let rc = unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) };
    if rc == -1 {
        return Err(std::io::Error::last_os_error());
    }
    unsafe { Ok((OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1]))) }
}

fn wait_for_registration(read_fd: OwnedFd) {
    let mut poll = libc::pollfd {
        fd: read_fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let ready = unsafe { libc::poll(&mut poll, 1, REGISTER_TIMEOUT_MS) };
    if ready <= 0 {
        return;
    }
    let mut file = std::fs::File::from(read_fd);
    let mut buf = [0u8; 16];
    while matches!(file.read(&mut buf), Ok(n) if n > 0) {}
}
