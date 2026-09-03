#![cfg(unix)]

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::PermissionsExt;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use herdr_quota::collector::Collector;
use tempfile::TempDir;

const REPORT: &str =
    r#"{"generatedAt":"2026-09-02T12:00:00.000Z","schemaVersion":5,"providers":[]}"#;

#[test]
fn child_dashboard_process() {
    if std::env::var_os("HERDR_QUOTA_TERMINAL_TEST_CHILD").is_none() {
        return;
    }
    let executable = std::env::var_os("HERDR_QUOTA_TEST_COLLECTOR").expect("collector path");
    let working = std::env::var_os("HERDR_QUOTA_TEST_WORKING").expect("collector directory");
    let collector = Collector::with_executable(executable, working, Duration::from_secs(2));
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime")
        .block_on(herdr_quota::ui::runtime::dashboard_with_collector(
            collector,
        ))
        .expect("dashboard exits cleanly");
}

struct DashboardChild {
    child: Child,
    master: File,
    slave: File,
    original_termios: libc::termios,
    output: Vec<u8>,
}

impl DashboardChild {
    fn spawn(directory: &TempDir, width: u16, height: u16) -> Self {
        let collector = directory.path().join("quota-axi");
        fs::write(
            &collector,
            format!("#!/bin/sh\nsleep 0.05\nprintf '%s' '{REPORT}'\n"),
        )
        .expect("write fake collector");
        let mut permissions = fs::metadata(&collector)
            .expect("collector metadata")
            .permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&collector, permissions).expect("make collector executable");

        let (master, slave) = open_pty(width, height);
        let original_termios = termios(&slave);
        set_nonblocking(&master);
        let child = Command::new(std::env::current_exe().expect("test executable"))
            .args(["--exact", "child_dashboard_process", "--nocapture"])
            .env("HERDR_QUOTA_TERMINAL_TEST_CHILD", "1")
            .env("HERDR_QUOTA_TEST_COLLECTOR", &collector)
            .env("HERDR_QUOTA_TEST_WORKING", directory.path())
            .env("XDG_CONFIG_HOME", directory.path().join("config"))
            .env("XDG_STATE_HOME", directory.path().join("state"))
            .env("NO_COLOR", "1")
            .stdin(Stdio::from(slave.try_clone().expect("clone slave stdin")))
            .stdout(Stdio::from(slave.try_clone().expect("clone slave stdout")))
            .stderr(Stdio::from(slave.try_clone().expect("clone slave stderr")))
            .spawn()
            .expect("spawn dashboard child");
        Self {
            child,
            master,
            slave,
            original_termios,
            output: Vec::new(),
        }
    }

    fn send(&mut self, bytes: &[u8]) {
        self.master.write_all(bytes).expect("write terminal input");
        thread::sleep(Duration::from_millis(25));
        self.drain();
    }

    fn resize(&mut self, width: u16, height: u16) {
        set_size(self.master.as_raw_fd(), width, height);
        // SAFETY: the child PID is live and SIGWINCH has no payload.
        assert_eq!(
            unsafe { libc::kill(self.child.id() as libc::pid_t, libc::SIGWINCH) },
            0
        );
        thread::sleep(Duration::from_millis(25));
        self.drain();
    }

    fn drain(&mut self) {
        let mut buffer = [0_u8; 8192];
        loop {
            match self.master.read(&mut buffer) {
                Ok(0) => return,
                Ok(count) => self.output.extend_from_slice(&buffer[..count]),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return,
                Err(error) if error.raw_os_error() == Some(libc::EIO) => return,
                Err(error) => panic!("read PTY output: {error}"),
            }
        }
    }

    fn wait_for(&mut self, label: &str, needle: &[u8]) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while !contains(&self.output, needle) {
            self.drain();
            assert!(
                Instant::now() < deadline,
                "dashboard evidence did not render: {label}"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn finish(mut self, signal: Option<libc::c_int>) -> Vec<u8> {
        if let Some(signal) = signal {
            // SAFETY: the child PID is live and the requested signal is a fixed test value.
            assert_eq!(
                unsafe { libc::kill(self.child.id() as libc::pid_t, signal) },
                0
            );
        }
        let deadline = Instant::now() + Duration::from_secs(4);
        loop {
            self.drain();
            if let Some(status) = self.child.try_wait().expect("wait for dashboard") {
                assert!(status.success(), "dashboard child exited {status}");
                break;
            }
            assert!(Instant::now() < deadline, "dashboard child did not exit");
            thread::sleep(Duration::from_millis(20));
        }
        self.drain();
        let restored = termios(&self.slave);
        let raw_mode_flags = libc::ICANON | libc::ECHO | libc::ISIG | libc::IEXTEN;
        assert_eq!(
            restored.c_lflag & raw_mode_flags,
            self.original_termios.c_lflag & raw_mode_flags
        );
        assert!(contains(&self.output, b"\x1b[?1049h"));
        assert!(contains(&self.output, b"\x1b[?25l"));
        assert!(contains(&self.output, b"\x1b[?25h"));
        assert!(contains(&self.output, b"\x1b[?1049l"));
        self.output
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[allow(clippy::unnecessary_mut_passed)] // macOS openpty requires a mutable winsize pointer.
fn open_pty(width: u16, height: u16) -> (File, File) {
    let mut master = -1;
    let mut slave = -1;
    let mut size = libc::winsize {
        ws_row: height,
        ws_col: width,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: all output pointers and the winsize pointer are valid for this call.
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut size,
            )
        },
        0
    );
    // SAFETY: successful openpty returned owned descriptors.
    unsafe { (File::from_raw_fd(master), File::from_raw_fd(slave)) }
}

fn set_size(fd: libc::c_int, width: u16, height: u16) {
    let size = libc::winsize {
        ws_row: height,
        ws_col: width,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: fd is a live PTY and size is a valid winsize value.
    assert_eq!(unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, &size) }, 0);
}

fn termios(file: &File) -> libc::termios {
    let mut value = std::mem::MaybeUninit::uninit();
    // SAFETY: file is a live PTY and tcgetattr initializes the output on success.
    assert_eq!(
        unsafe { libc::tcgetattr(file.as_raw_fd(), value.as_mut_ptr()) },
        0
    );
    // SAFETY: tcgetattr succeeded and initialized the value.
    unsafe { value.assume_init() }
}

fn set_nonblocking(file: &File) {
    // SAFETY: file owns a live descriptor and fcntl does not retain pointers.
    let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
    assert!(flags >= 0);
    // SAFETY: descriptor and flag bits are valid.
    assert_eq!(
        unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) },
        0
    );
}

#[test]
fn pty_journey_handles_resize_fragmented_escape_modals_refresh_and_quit() {
    let directory = TempDir::new().expect("temporary directory");
    let mut dashboard = DashboardChild::spawn(&directory, 36, 23);
    dashboard.wait_for("terminal enter", b"\x1b[?25l");
    dashboard.send(b"p");
    dashboard.wait_for("Preferences", b"Preferences");
    dashboard.send(b"\x1b");
    dashboard.send(b"[");
    dashboard.send(b"B");
    dashboard.send(b"xn");
    dashboard.send(b"c");
    dashboard.resize(20, 12);
    dashboard.send(b"rrr");
    dashboard.send(b"pjjjjjjjjjjjj\r");
    thread::sleep(Duration::from_millis(150));
    dashboard.drain();
    dashboard.send(b"y");
    dashboard.send(b"c");
    dashboard.send(b"a\r");
    dashboard.send(b"q");
    let output = dashboard.finish(None);
    assert!(contains(&output, b"Preferences"));
}

#[test]
fn sigint_and_sigterm_restore_raw_cursor_and_alternate_screen() {
    for signal in [libc::SIGINT, libc::SIGTERM] {
        let directory = TempDir::new().expect("temporary directory");
        let mut dashboard = DashboardChild::spawn(&directory, 20, 12);
        dashboard.wait_for("terminal enter", b"\x1b[?25l");
        dashboard.finish(Some(signal));
    }
}
