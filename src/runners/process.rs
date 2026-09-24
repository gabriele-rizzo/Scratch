use std::{
    io::{self, BufRead, BufReader, Read},
    process::{Child, Command, ExitStatus, Stdio},
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{Duration, Instant},
};

pub enum RunStatus {
    Running,
    Exited(ExitStatus, Duration),
}

/// A running (or finished) program whose output is collected line by line.
///
/// stdout and stderr share one pseudo-terminal, so lines arrive in the order the
/// program wrote them, and programs line-buffer (and colorize) as they would in a
/// real terminal.
pub struct Process {
    child: Child,
    output: Receiver<String>,
    /// Raw output lines, possibly containing ANSI escape codes.
    pub lines: Vec<String>,
    pub started: Instant,
    pub status: RunStatus,
}

impl Process {
    /// Spawns `command` with a terminal `columns` wide.
    pub fn spawn(mut command: Command, columns: u16) -> io::Result<Self> {
        let (reader, stdout, stderr) = output_channel(columns)?;

        // Its own process group, so stopping it also stops anything it started (like a
        // compiler still running under `sh`).
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);

        let child = command
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr)
            .spawn()?;

        // Closes our copies of the terminal's write end, so the reader sees EOF once the
        // program (and anything it started) exits.
        drop(command);

        let (tx, rx) = mpsc::channel();
        forward(reader, tx);

        Ok(Self {
            child,
            output: rx,
            lines: Vec::new(),
            started: Instant::now(),
            status: RunStatus::Running,
        })
    }

    /// Collects new output and checks whether the program has exited.
    pub fn poll(&mut self) {
        self.lines.extend(self.output.try_iter());

        if let RunStatus::Running = self.status
            && let Ok(Some(status)) = self.child.try_wait()
        {
            self.status = RunStatus::Exited(status, self.started.elapsed());
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if let RunStatus::Running = self.status {
            kill_group(&mut self.child);
            let _ = self.child.wait();
        }
    }
}

/// Kills the child's whole process group.
#[cfg(unix)]
fn kill_group(child: &mut Child) {
    // The child leads its group, so the group id is its pid. It hasn't been reaped yet,
    // so the id can't have been reused.
    // SAFETY: killpg has no memory-safety preconditions.
    let result = unsafe { libc::killpg(child.id() as libc::pid_t, libc::SIGKILL) };

    if result != 0 {
        let _ = child.kill();
    }
}

#[cfg(not(unix))]
fn kill_group(child: &mut Child) {
    let _ = child.kill();
}

/// Opens a pseudo-terminal and returns its read end plus stdout/stderr for the child.
#[cfg(unix)]
fn output_channel(columns: u16) -> io::Result<(std::fs::File, Stdio, Stdio)> {
    use std::os::fd::{FromRawFd, OwnedFd};

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
    let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };

    // Keep the child from inheriting the read end.
    // SAFETY: `master` is a valid open descriptor.
    unsafe {
        libc::fcntl(
            std::os::fd::AsRawFd::as_raw_fd(&master),
            libc::F_SETFD,
            libc::FD_CLOEXEC,
        )
    };

    let stderr = slave.try_clone()?;
    Ok((master.into(), slave.into(), stderr.into()))
}

/// Without ptys, stdout and stderr are merged through a single pipe.
#[cfg(not(unix))]
fn output_channel(_columns: u16) -> io::Result<(io::PipeReader, Stdio, Stdio)> {
    let (reader, writer) = io::pipe()?;
    let stderr = writer.try_clone()?;
    Ok((reader, writer.into(), stderr.into()))
}

fn forward(pipe: impl Read + Send + 'static, tx: Sender<String>) {
    thread::spawn(move || {
        let mut reader = BufReader::new(pipe);
        let mut buffer = Vec::new();

        // A pty reports EIO instead of EOF once the program exits, so any error ends it.
        while let Ok(read) = reader.read_until(b'\n', &mut buffer) {
            if read == 0 {
                break;
            }

            let text = String::from_utf8_lossy(&buffer);
            let text = text.trim_end_matches(['\n', '\r']);
            // A bare `\r` returns to the start of the line (progress bars), so only the
            // last segment is visible.
            let line = text
                .rsplit('\r')
                .next()
                .unwrap_or_default()
                .replace('\t', "    ");

            if tx.send(line).is_err() {
                break;
            }

            buffer.clear();
        }
    });
}
