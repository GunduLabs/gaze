// SPDX-FileCopyrightText: 2026 Gundu Labs
// SPDX-License-Identifier: GPL-3.0-or-later

use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};

use console::style;

// pkttyagent signals that it has registered with polkit by closing this fd, so the parent waits
// for EOF rather than for any data. dup2 in pre_exec strips O_CLOEXEC, letting the child inherit it.
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::Instant;

    fn is_cloexec(fd: RawFd) -> bool {
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        assert_ne!(flags, -1, "F_GETFD failed on fd {fd}");
        flags & libc::FD_CLOEXEC != 0
    }

    #[test]
    fn the_notify_fd_does_not_collide_with_the_standard_streams() {
        assert_eq!(NOTIFY_FD, 3);
        assert!(NOTIFY_FD > libc::STDERR_FILENO);
    }

    #[test]
    fn pipe_cloexec_hands_back_a_connected_pair() {
        let (read_fd, write_fd) = pipe_cloexec().unwrap();

        let mut writer = std::fs::File::from(write_fd);
        writer.write_all(b"registered").unwrap();
        drop(writer);

        let mut reader = std::fs::File::from(read_fd);
        let mut got = String::new();
        reader.read_to_string(&mut got).unwrap();
        assert_eq!(got, "registered");
    }

    #[test]
    fn pipe_cloexec_marks_both_ends_close_on_exec() {
        let (read_fd, write_fd) = pipe_cloexec().unwrap();

        assert!(
            is_cloexec(read_fd.as_raw_fd()),
            "the read end must not leak into pkttyagent"
        );
        assert!(
            is_cloexec(write_fd.as_raw_fd()),
            "the write end is duplicated onto fd 3 by pre_exec, so it must start close-on-exec"
        );
    }

    #[test]
    fn waiting_for_registration_returns_as_soon_as_the_agent_closes_the_fd() {
        let (read_fd, write_fd) = pipe_cloexec().unwrap();
        drop(write_fd);

        let started = Instant::now();
        wait_for_registration(read_fd);

        assert!(
            started.elapsed().as_millis() < u128::try_from(REGISTER_TIMEOUT_MS).unwrap(),
            "EOF should end the wait well before the {REGISTER_TIMEOUT_MS}ms timeout"
        );
    }

    #[test]
    fn waiting_for_registration_drains_a_chatty_agent_before_returning() {
        let (read_fd, write_fd) = pipe_cloexec().unwrap();
        let mut writer = std::fs::File::from(write_fd);
        // More than the 16-byte read buffer, so the drain loop has to go round more than once.
        writer.write_all(&[b'x'; 64]).unwrap();
        drop(writer);

        let started = Instant::now();
        wait_for_registration(read_fd);

        assert!(started.elapsed().as_millis() < u128::try_from(REGISTER_TIMEOUT_MS).unwrap());
    }

    #[test]
    fn an_agent_that_never_started_drops_without_reaping_anything() {
        let agent = PolkitAgent { child: None };
        drop(agent);
    }
}
