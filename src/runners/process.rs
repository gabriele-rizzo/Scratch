use std::{
    io::{self, Read, Write},
    process::{Child, Command, ExitStatus, Stdio},
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{Duration, Instant},
};

pub enum RunStatus {
    Running,
    Exited(ExitStatus, Duration),
}

/// A running (or finished) program with its output and a way to send it input.
///
/// stdin, stdout and stderr share one pseudo-terminal, so output arrives in the order
/// the program wrote it, and programs line-buffer (and colorize) as they would in a
/// real terminal.
pub struct Process {
    child: Child,
    output: Receiver<Vec<u8>>,
    /// Writes to the program's stdin; `None` once input is closed.
    input: Option<Box<dyn Write + Send>>,
    /// Output after the last newline, such as a prompt waiting for input.
    pending: Vec<u8>,
    /// Complete output lines, possibly containing ANSI escape codes.
    pub lines: Vec<String>,
    interrupted: bool,
    pub started: Instant,
    pub status: RunStatus,
}

impl Process {
    /// Spawns `command` with a terminal `columns` wide.
    pub fn spawn(mut command: Command, columns: u16) -> io::Result<Self> {
        let channel = Channel::open(columns)?;

        // Its own process group, so stopping it also stops anything it started (like a
        // compiler still running under `sh`).
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);

        let mut child = command
            .stdin(channel.stdin)
            .stdout(channel.stdout)
            .stderr(channel.stderr)
            .spawn()?;

        // Closes our copies of the terminal's program side, so the reader sees EOF once
        // the program (and anything it started) exits.
        drop(command);

        let input = channel.writer.or_else(|| {
            let stdin = child.stdin.take()?;
            Some(Box::new(stdin) as Box<dyn Write + Send>)
        });

        let (tx, rx) = mpsc::channel();
        forward(channel.reader, tx);

        Ok(Self {
            child,
            output: rx,
            input,
            pending: Vec::new(),
            lines: Vec::new(),
            interrupted: false,
            started: Instant::now(),
            status: RunStatus::Running,
        })
    }

    pub fn is_running(&self) -> bool {
        matches!(self.status, RunStatus::Running)
    }

    /// Collects new output and checks whether the program has exited.
    pub fn poll(&mut self) {
        while let Ok(chunk) = self.output.try_recv() {
            self.push_output(&chunk);
        }

        if self.is_running()
            && let Ok(Some(status)) = self.child.try_wait()
        {
            self.status = RunStatus::Exited(status, self.started.elapsed());
        }
    }

    fn push_output(&mut self, bytes: &[u8]) {
        self.pending.extend_from_slice(bytes);

        while let Some(end) = self.pending.iter().position(|&byte| byte == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=end).collect();
            self.lines.push(clean(&String::from_utf8_lossy(&line)));
        }
    }

    /// The output after the last newline, e.g. `Name: ` while waiting for input.
    pub fn partial(&self) -> String {
        // Leave out a character still being received.
        let complete = match std::str::from_utf8(&self.pending) {
            Ok(_) => self.pending.len(),
            Err(err) if err.error_len().is_none() => err.valid_up_to(),
            Err(_) => self.pending.len(),
        };

        clean(&String::from_utf8_lossy(&self.pending[..complete]))
    }

    /// Sends `text` and a newline to the program's stdin.
    pub fn send_line(&mut self, text: &str) {
        let Some(input) = &mut self.input else { return };

        if input.write_all(format!("{text}\n").as_bytes()).is_ok() {
            let _ = input.flush();
            // The terminal doesn't echo, so show the input where a terminal would.
            self.push_output(format!("{text}\n").as_bytes());
        }
    }

    /// Closes the program's stdin, like ctrl+d in a terminal.
    pub fn send_eof(&mut self) {
        #[cfg(unix)]
        if let Some(input) = &mut self.input {
            // With the terminal in line mode, ^D at the start of a line reads as EOF.
            let _ = input.write_all(b"\x04");
            let _ = input.flush();
            return;
        }

        self.input = None;
    }

    /// Asks the program to stop, like ctrl+c in a terminal. A second call kills it.
    pub fn interrupt(&mut self) {
        if !self.is_running() {
            return;
        }

        if self.interrupted {
            kill_group(&mut self.child);
        } else {
            self.interrupted = true;
            signal_group(&mut self.child);
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if self.is_running() {
            kill_group(&mut self.child);
            let _ = self.child.wait();
        }
    }
}

/// Cleans one line of terminal output for display.
fn clean(text: &str) -> String {
    let text = text.trim_end_matches(['\n', '\r']);
    // A bare `\r` returns to the start of the line (progress bars), so only the last
    // segment is visible.
    let text = text.rsplit('\r').next().unwrap_or_default();

    let mut line = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\x08' => {
                line.pop();
            }
            '\t' => line.push_str("    "),
            c => line.push(c),
        }
    }
    line
}

/// Where the program's stdio goes, and our ends of it.
struct Channel {
    reader: Box<dyn Read + Send>,
    /// `None` when stdin is a pipe taken from the child after spawning.
    writer: Option<Box<dyn Write + Send>>,
    stdin: Stdio,
    stdout: Stdio,
    stderr: Stdio,
}

impl Channel {
    /// A pseudo-terminal for all three streams.
    #[cfg(unix)]
    fn open(columns: u16) -> io::Result<Self> {
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

        let mut master = -1;
        let mut slave = -1;
        let mut size = libc::winsize {
            ws_row: 24,
            ws_col: columns.max(20),
            ws_xpixel: 0,
            ws_ypixel: 0,
        };

        // SAFETY: all pointers are valid for the call; the name and termios are optional.
        let result = unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut size,
            )
        };
        if result != 0 {
            return Err(io::Error::last_os_error());
        }

        // SAFETY: openpty succeeded, so both descriptors are open and owned by us.
        let (master, slave) =
            unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };

        // SAFETY: both descriptors are valid, and `termios` is fully written by
        // tcgetattr before it is read.
        unsafe {
            // Keep the child from inheriting our end.
            libc::fcntl(master.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC);

            // No echo: input is shown by us, with its own editing.
            let mut termios = std::mem::zeroed::<libc::termios>();
            if libc::tcgetattr(slave.as_raw_fd(), &mut termios) == 0 {
                termios.c_lflag &= !(libc::ECHO | libc::ECHONL);
                libc::tcsetattr(slave.as_raw_fd(), libc::TCSANOW, &termios);
            }
        }

        let writer = master.try_clone()?;
        let stdout = slave.try_clone()?;
        let stderr = slave.try_clone()?;

        Ok(Self {
            reader: Box::new(std::fs::File::from(master)),
            writer: Some(Box::new(std::fs::File::from(writer))),
            stdin: slave.into(),
            stdout: stdout.into(),
            stderr: stderr.into(),
        })
    }

    /// Without ptys: stdout and stderr merged through one pipe, stdin a pipe.
    #[cfg(not(unix))]
    fn open(_columns: u16) -> io::Result<Self> {
        let (reader, writer) = io::pipe()?;
        let stderr = writer.try_clone()?;

        Ok(Self {
            reader: Box::new(reader),
            writer: None,
            stdin: Stdio::piped(),
            stdout: writer.into(),
            stderr: stderr.into(),
        })
    }
}

fn forward(mut reader: Box<dyn Read + Send>, tx: Sender<Vec<u8>>) {
    thread::spawn(move || {
        let mut buffer = [0; 4096];

        // A pty reports EIO instead of EOF once the program exits, so any error ends it.
        while let Ok(read) = reader.read(&mut buffer) {
            if read == 0 || tx.send(buffer[..read].to_vec()).is_err() {
                break;
            }
        }
    });
}

/// Sends SIGINT to the child's process group.
#[cfg(unix)]
fn signal_group(child: &mut Child) {
    // SAFETY: killpg has no memory-safety preconditions. The child leads its group and
    // hasn't been reaped, so the id can't have been reused.
    unsafe { libc::killpg(child.id() as libc::pid_t, libc::SIGINT) };
}

#[cfg(not(unix))]
fn signal_group(child: &mut Child) {
    let _ = child.kill();
}

/// Kills the child's whole process group.
#[cfg(unix)]
fn kill_group(child: &mut Child) {
    // SAFETY: as in `signal_group`.
    let result = unsafe { libc::killpg(child.id() as libc::pid_t, libc::SIGKILL) };

    if result != 0 {
        let _ = child.kill();
    }
}

#[cfg(not(unix))]
fn kill_group(child: &mut Child) {
    let _ = child.kill();
}
