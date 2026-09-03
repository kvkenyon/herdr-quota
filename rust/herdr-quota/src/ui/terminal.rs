//! Pane-owned terminal setup, restoration, and cancellable raw input.

use std::io::{self, Stdout, Write};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};

use crossterm::{
    cursor::{Hide, Show},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use tokio::sync::mpsc;

pub(crate) trait RawMode {
    fn enable(&mut self) -> io::Result<()>;
    fn disable(&mut self) -> io::Result<()>;
}

pub(crate) struct SystemRawMode;

impl RawMode for SystemRawMode {
    fn enable(&mut self) -> io::Result<()> {
        enable_raw_mode()
    }

    fn disable(&mut self) -> io::Result<()> {
        disable_raw_mode()
    }
}

/// Restores every terminal feature it acquired, including during unwinding.
pub(crate) struct TerminalGuard<W: Write, R: RawMode = SystemRawMode> {
    writer: W,
    raw_mode: R,
    raw: bool,
    alternate: bool,
    cursor_hidden: bool,
}

impl TerminalGuard<Stdout, SystemRawMode> {
    pub(crate) fn enter_stdout() -> io::Result<Self> {
        Self::enter_with(io::stdout(), SystemRawMode)
    }
}

impl<W: Write, R: RawMode> TerminalGuard<W, R> {
    fn enter_with(writer: W, raw_mode: R) -> io::Result<Self> {
        let mut session = Self {
            writer,
            raw_mode,
            raw: false,
            alternate: false,
            cursor_hidden: false,
        };
        session.raw_mode.enable()?;
        session.raw = true;
        session.alternate = true;
        execute!(session.writer, EnterAlternateScreen)?;
        session.cursor_hidden = true;
        execute!(session.writer, Hide)?;
        Ok(session)
    }

    pub(crate) fn writer_mut(&mut self) -> &mut W {
        &mut self.writer
    }

    pub(crate) fn restore(&mut self) -> io::Result<()> {
        let mut first_error = None;
        if self.cursor_hidden {
            self.cursor_hidden = false;
            if let Err(error) = execute!(self.writer, Show) {
                first_error = Some(error);
            }
        }
        if self.alternate {
            self.alternate = false;
            if let Err(error) = execute!(self.writer, LeaveAlternateScreen)
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        if self.raw {
            self.raw = false;
            if let Err(error) = self.raw_mode.disable()
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

impl<W: Write, R: RawMode> Drop for TerminalGuard<W, R> {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

/// A cancellable reader scoped to one dashboard process.
pub(crate) struct RawInput {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl RawInput {
    pub(crate) fn open() -> (Self, mpsc::UnboundedReceiver<io::Result<Vec<u8>>>) {
        let (sender, receiver) = mpsc::unbounded_channel();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let thread = thread::spawn(move || read_loop(worker_stop, sender));
        (
            Self {
                stop,
                thread: Some(thread),
            },
            receiver,
        )
    }
}

impl Drop for RawInput {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(unix)]
fn read_loop(stop: Arc<AtomicBool>, sender: mpsc::UnboundedSender<io::Result<Vec<u8>>>) {
    while !stop.load(Ordering::Acquire) {
        let mut descriptor = libc::pollfd {
            fd: libc::STDIN_FILENO,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: `descriptor` is valid for one element and the timeout is bounded.
        let ready = unsafe { libc::poll(&mut descriptor, 1, 50) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            let _ = sender.send(Err(error));
            return;
        }
        if ready == 0 {
            continue;
        }
        if descriptor.revents & libc::POLLIN == 0 {
            let _ = sender.send(Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "terminal input closed",
            )));
            return;
        }
        let mut bytes = [0_u8; 1024];
        // SAFETY: `bytes` is writable for its full length and stdin is process-owned.
        let count =
            unsafe { libc::read(libc::STDIN_FILENO, bytes.as_mut_ptr().cast(), bytes.len()) };
        if count > 0 {
            if sender.send(Ok(bytes[..count as usize].to_vec())).is_err() {
                return;
            }
        } else if count == 0 {
            let _ = sender.send(Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "terminal input closed",
            )));
            return;
        } else {
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                let _ = sender.send(Err(error));
                return;
            }
        }
    }
}

#[cfg(not(unix))]
fn read_loop(_: Arc<AtomicBool>, sender: mpsc::UnboundedSender<io::Result<Vec<u8>>>) {
    let _ = sender.send(Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "interactive dashboard requires a POSIX terminal",
    )));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct RawState {
        enables: usize,
        disables: usize,
    }

    struct FakeRaw(Arc<Mutex<RawState>>);

    impl RawMode for FakeRaw {
        fn enable(&mut self) -> io::Result<()> {
            self.0.lock().expect("raw state").enables += 1;
            Ok(())
        }

        fn disable(&mut self) -> io::Result<()> {
            self.0.lock().expect("raw state").disables += 1;
            Ok(())
        }
    }

    struct FailingWriter;

    impl Write for FailingWriter {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("test write failure"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("test flush failure"))
        }
    }

    #[test]
    fn explicit_restore_is_complete_and_idempotent() {
        let raw = Arc::new(Mutex::new(RawState::default()));
        let mut guard = TerminalGuard::enter_with(Vec::new(), FakeRaw(Arc::clone(&raw)))
            .expect("terminal enters");
        guard.restore().expect("terminal restores");
        guard.restore().expect("second restore is inert");
        let output = String::from_utf8(guard.writer.clone()).expect("ANSI is UTF-8");
        assert!(output.contains("\x1b[?1049h"));
        assert!(output.contains("\x1b[?25l"));
        assert!(output.contains("\x1b[?25h"));
        assert!(output.contains("\x1b[?1049l"));
        let raw = raw.lock().expect("raw state");
        assert_eq!((raw.enables, raw.disables), (1, 1));
    }

    #[test]
    fn drop_restores_during_unwind() {
        let raw = Arc::new(Mutex::new(RawState::default()));
        let unwind_raw = Arc::clone(&raw);
        let result = std::panic::catch_unwind(move || {
            let _guard = TerminalGuard::enter_with(Vec::new(), FakeRaw(unwind_raw))
                .expect("terminal enters");
            panic!("test panic");
        });
        assert!(result.is_err());
        let raw = raw.lock().expect("raw state");
        assert_eq!((raw.enables, raw.disables), (1, 1));
    }

    #[test]
    fn partial_setup_error_still_disables_raw_mode() {
        let raw = Arc::new(Mutex::new(RawState::default()));
        assert!(TerminalGuard::enter_with(FailingWriter, FakeRaw(Arc::clone(&raw))).is_err());
        let raw = raw.lock().expect("raw state");
        assert_eq!((raw.enables, raw.disables), (1, 1));
    }

    #[test]
    fn escape_timeout_is_not_a_fast_refresh_interval() {
        assert_eq!(
            super::super::keys::ESCAPE_FRAGMENT_TIMEOUT,
            std::time::Duration::from_millis(100)
        );
    }
}
