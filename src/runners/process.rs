use std::{
    collections::VecDeque,
    io::{self, Read, Write},
    process::{Child, Command, ExitStatus, Stdio},
    sync::mpsc::{self, Receiver, SyncSender},
    thread,
    time::{Duration, Instant},
};

/// Lines kept for scrollback; older ones are dropped, like a terminal.
pub const SCROLLBACK: usize = 10_000;

/// Output chunks in flight between the reader thread and the UI. When the UI falls
/// behind, the reader waits, the terminal's buffer fills, and the program pauses on
/// its next write, just like with a slow terminal.
const CHANNEL_CHUNKS: usize = 16;

/// How long one `poll` may spend on output, so the UI stays responsive however
/// fast the program prints.
const POLL_BUDGET: Duration = Duration::from_millis(10);

/// A line longer than this without a newline is cut, so a program printing forever
/// without one can't grow memory without bound.
const MAX_LINE_BYTES: usize = 16 * 1024;

/// The dimensions of the program's terminal, in cells.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TerminalSize {
    pub columns: u16,
    pub rows: u16,
}

impl TerminalSize {
    /// Keeps sizes usable for programs that lay out output by width.
    fn clamped(self) -> Self {
        Self {
            columns: self.columns.max(20),
            rows: self.rows.max(1),
        }
    }
}

/// A handle for resizing the program's terminal.
#[cfg(unix)]
type Terminal = std::os::fd::OwnedFd;
#[cfg(not(unix))]
type Terminal = ();

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
    /// The last `SCROLLBACK` complete output lines, possibly containing ANSI
    /// escape codes.
    pub lines: VecDeque<String>,
    /// How many lines were dropped from the front of `lines`.
    pub dropped: usize,
    interrupted: bool,
    terminal: Option<Terminal>,
    size: TerminalSize,
    pub started: Instant,
    pub status: RunStatus,
}

impl Process {
    /// Spawns `command` with a terminal of the given size.
    pub fn spawn(mut command: Command, size: TerminalSize) -> io::Result<Self> {
        let size = size.clamped();
        let channel = Channel::open(size)?;

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

        let (tx, rx) = mpsc::sync_channel(CHANNEL_CHUNKS);
        forward(channel.reader, tx);

        Ok(Self {
            child,
            output: rx,
            input,
            pending: Vec::new(),
            lines: VecDeque::new(),
            dropped: 0,
            interrupted: false,
            terminal: channel.terminal,
            size,
            started: Instant::now(),
            status: RunStatus::Running,
        })
    }

    pub fn is_running(&self) -> bool {
        matches!(self.status, RunStatus::Running)
    }

    /// Collects new output (for at most `POLL_BUDGET`) and checks whether the
    /// program has exited.
    pub fn poll(&mut self) {
        let deadline = Instant::now() + POLL_BUDGET;

        while Instant::now() < deadline
            && let Ok(chunk) = self.output.try_recv()
        {
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

        // Split off every complete line, then remove them from `pending` at once.
        let mut start = 0;
        while let Some(end) = self.pending[start..].iter().position(|&byte| byte == b'\n') {
            let line = clean(&String::from_utf8_lossy(&self.pending[start..=start + end]));
            self.push_line(line);
            start += end + 1;
        }
        self.pending.drain(..start);

        if self.pending.len() > MAX_LINE_BYTES {
            let cut = complete_utf8_len(&self.pending[..MAX_LINE_BYTES]);
            let line: Vec<u8> = self.pending.drain(..cut).collect();
            self.push_line(clean(&String::from_utf8_lossy(&line)));
        }
    }

    fn push_line(&mut self, line: String) {
        self.lines.push_back(line);

        if self.lines.len() > SCROLLBACK {
            self.lines.pop_front();
            self.dropped += 1;
        }
    }

    /// The output after the last newline, e.g. `Name: ` while waiting for input.
    pub fn partial(&self) -> String {
        let complete = complete_utf8_len(&self.pending);
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

    /// Resizes the program's terminal and tells the program, like a terminal window
    /// being resized.
    pub fn resize(&mut self, size: TerminalSize) {
        let size = size.clamped();
        if size == self.size || !self.is_running() {
            return;
        }
        self.size = size;

        #[cfg(unix)]
        if let Some(terminal) = &self.terminal {
            set_size(terminal, size);
            // The program isn't in the terminal's foreground group (it's not its
            // controlling terminal), so the kernel won't send this itself.
            // SAFETY: as in `signal_group`.
            unsafe { libc::killpg(self.child.id() as libc::pid_t, libc::SIGWINCH) };
        }
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

/// Length of `bytes` without a trailing character that's still being received.
fn complete_utf8_len(bytes: &[u8]) -> usize {
    match std::str::from_utf8(bytes) {
        Err(err) if err.error_len().is_none() => err.valid_up_to(),
        _ => bytes.len(),
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
    terminal: Option<Terminal>,
}

impl Channel {
    /// A pseudo-terminal for all three streams.
    #[cfg(unix)]
    fn open(size: TerminalSize) -> io::Result<Self> {
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

        let mut master = -1;
        let mut slave = -1;
        let mut size = winsize(size);

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
        let terminal = master.try_clone()?;
        let stdout = slave.try_clone()?;
        let stderr = slave.try_clone()?;

        Ok(Self {
            reader: Box::new(std::fs::File::from(master)),
            writer: Some(Box::new(std::fs::File::from(writer))),
            stdin: slave.into(),
            stdout: stdout.into(),
            stderr: stderr.into(),
            terminal: Some(terminal),
        })
    }

    /// Without ptys: stdout and stderr merged through one pipe, stdin a pipe.
    #[cfg(not(unix))]
    fn open(_size: TerminalSize) -> io::Result<Self> {
        let (reader, writer) = io::pipe()?;
        let stderr = writer.try_clone()?;

        Ok(Self {
            reader: Box::new(reader),
            writer: None,
            stdin: Stdio::piped(),
            stdout: writer.into(),
            stderr: stderr.into(),
            terminal: None,
        })
    }
}

#[cfg(unix)]
fn winsize(size: TerminalSize) -> libc::winsize {
    libc::winsize {
        ws_row: size.rows,
        ws_col: size.columns,
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}

#[cfg(unix)]
fn set_size(terminal: &Terminal, size: TerminalSize) {
    use std::os::fd::AsRawFd;

    let size = winsize(size);
    // SAFETY: `terminal` is an open pty and `size` is a valid winsize.
    unsafe { libc::ioctl(terminal.as_raw_fd(), libc::TIOCSWINSZ, &size) };
}

fn forward(mut reader: Box<dyn Read + Send>, tx: SyncSender<Vec<u8>>) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_keeps_the_last_carriage_return_segment() {
        assert_eq!(clean("progress 10%\rprogress 100%\r\n"), "progress 100%");
    }

    #[test]
    fn clean_applies_backspaces_and_expands_tabs() {
        assert_eq!(clean("abc\x08\x08d\n"), "ad");
        assert_eq!(clean("a\tb"), "a    b");
        assert_eq!(clean("\x08\x08x"), "x");
    }

    #[test]
    fn complete_utf8_len_holds_back_a_split_character() {
        let bytes = "é".as_bytes();
        assert_eq!(complete_utf8_len(b"abc"), 3);
        assert_eq!(complete_utf8_len(&[b'a', bytes[0]]), 1);
        // Invalid bytes aren't held back forever.
        assert_eq!(complete_utf8_len(&[b'a', 0xff, b'b']), 3);
    }
}

/// These run real programs on a pseudo-terminal.
#[cfg(all(test, unix))]
mod process_tests {
    use std::time::Duration;

    use super::*;

    const SIZE: TerminalSize = TerminalSize {
        columns: 80,
        rows: 24,
    };

    fn sh(script: &str) -> Process {
        let mut command = Command::new("sh");
        command.arg("-c").arg(script);
        Process::spawn(command, SIZE).unwrap()
    }

    /// Polls until `done` holds, failing after a few seconds.
    fn wait_for(process: &mut Process, done: impl Fn(&Process) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            process.poll();
            if done(process) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out; output: {:?}",
                process.lines
            );
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn exit_code(process: &Process) -> Option<i32> {
        match &process.status {
            RunStatus::Exited(status, _) => status.code(),
            RunStatus::Running => None,
        }
    }

    /// Waits for exit and for the output that's still in flight.
    fn wait_for_exit(process: &mut Process, lines: usize) {
        wait_for(process, |p| !p.is_running() && p.lines.len() >= lines);
    }

    #[test]
    fn keeps_stdout_and_stderr_in_order() {
        let mut process = sh("for i in 1 2 3; do echo out$i; echo err$i >&2; done");
        wait_for_exit(&mut process, 6);
        assert_eq!(
            process.lines,
            ["out1", "err1", "out2", "err2", "out3", "err3"]
        );
    }

    #[test]
    fn line_buffers_like_a_terminal() {
        // Pipes would make Python buffer stdout until exit, putting stderr first.
        let mut command = Command::new("python3");
        command.args([
            "-c",
            "import sys\nfor i in range(3):\n print('out', i)\n print('err', i, file=sys.stderr)",
        ]);
        let Ok(mut process) = Process::spawn(command, SIZE) else {
            return; // python3 isn't installed
        };

        wait_for_exit(&mut process, 6);
        assert_eq!(
            process.lines,
            ["out 0", "err 0", "out 1", "err 1", "out 2", "err 2"]
        );
    }

    #[test]
    fn shows_prompts_and_echoes_input() {
        let mut process = sh("printf 'Name: '; read name; echo \"Hi $name\"");
        wait_for(&mut process, |p| p.partial() == "Name: ");

        process.send_line("Bob");
        wait_for_exit(&mut process, 2);
        assert_eq!(process.lines, ["Name: Bob", "Hi Bob"]);
        assert_eq!(process.partial(), "");
    }

    #[test]
    fn end_of_input_closes_stdin() {
        let mut process = sh("cat");
        process.send_line("a");
        process.send_eof();
        // Our echo of the input, then cat's copy of it.
        wait_for_exit(&mut process, 2);
        assert_eq!(process.lines, ["a", "a"]);
        assert_eq!(exit_code(&process), Some(0));
    }

    #[test]
    fn interrupt_sends_sigint() {
        let mut process =
            sh("trap 'echo caught; exit 3' INT; echo ready; while :; do sleep 0.05; done");
        wait_for(&mut process, |p| p.lines.iter().any(|line| line == "ready"));

        process.interrupt();
        wait_for_exit(&mut process, 2);
        assert_eq!(process.lines, ["ready", "caught"]);
        assert_eq!(exit_code(&process), Some(3));
    }

    #[test]
    fn a_second_interrupt_kills() {
        let mut process = sh("trap '' INT; echo ready; while :; do sleep 0.05; done");
        wait_for(&mut process, |p| p.lines.iter().any(|line| line == "ready"));

        process.interrupt();
        thread::sleep(Duration::from_millis(100));
        process.poll();
        assert!(process.is_running(), "SIGINT should be ignored");

        process.interrupt();
        wait_for(&mut process, |p| !p.is_running());
        assert_eq!(exit_code(&process), None);
    }

    #[test]
    fn dropping_kills_the_whole_process_group() {
        let mut process = sh("sleep 30 & echo $!; wait");
        wait_for(&mut process, |p| !p.lines.is_empty());
        let sleep: libc::pid_t = process.lines[0].parse().unwrap();

        drop(process);

        // The orphaned `sleep` is killed too (and reaped by init shortly after).
        let deadline = Instant::now() + Duration::from_secs(5);
        // SAFETY: kill with signal 0 only checks whether the process exists.
        while unsafe { libc::kill(sleep, 0) } == 0 {
            assert!(Instant::now() < deadline, "sleep {sleep} survived");
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn programs_see_a_terminal_of_the_given_width() {
        let mut command = Command::new("sh");
        command.args(["-c", "stty size"]);
        let mut process = Process::spawn(
            command,
            TerminalSize {
                columns: 123,
                rows: 45,
            },
        )
        .unwrap();
        wait_for_exit(&mut process, 1);
        assert_eq!(process.lines, ["45 123"]);
    }

    #[test]
    fn resizing_updates_the_terminal_and_signals_the_program() {
        let mut process = sh("trap 'stty size' WINCH; echo ready; while :; do sleep 0.05; done");
        wait_for(&mut process, |p| p.lines.iter().any(|line| line == "ready"));

        process.resize(TerminalSize {
            columns: 100,
            rows: 30,
        });
        wait_for(&mut process, |p| {
            p.lines.iter().any(|line| line == "30 100")
        });
    }

    #[test]
    fn resizing_to_the_same_size_does_nothing() {
        let mut process = sh("trap 'echo winch' WINCH; echo ready; while :; do sleep 0.05; done");
        wait_for(&mut process, |p| p.lines.iter().any(|line| line == "ready"));

        process.resize(SIZE);
        thread::sleep(Duration::from_millis(200));
        process.poll();
        assert_eq!(process.lines, ["ready"]);
    }

    #[test]
    fn keeps_only_the_last_scrollback_lines() {
        let mut process = sh(&format!("seq 1 {}", SCROLLBACK + 500));
        wait_for(&mut process, |p| {
            !p.is_running() && p.dropped + p.lines.len() == SCROLLBACK + 500
        });

        assert_eq!(process.lines.len(), SCROLLBACK);
        assert_eq!(process.dropped, 500);
        assert_eq!(process.lines[0], "501");
    }

    #[test]
    fn cuts_endless_lines() {
        let mut process = sh("head -c 40000 /dev/zero | tr '\\0' x; echo");
        wait_for(&mut process, |p| {
            !p.is_running() && p.lines.iter().map(String::len).sum::<usize>() == 40000
        });

        assert!(
            process
                .lines
                .iter()
                .all(|line| line.len() <= MAX_LINE_BYTES)
        );
        assert_eq!(process.lines.len(), 3);
    }

    #[test]
    fn a_flood_of_output_does_not_block_polling() {
        let mut process = sh("yes");
        wait_for(&mut process, |p| !p.lines.is_empty());

        // Each poll stops after its budget, however fast `yes` prints.
        let started = Instant::now();
        for _ in 0..20 {
            process.poll();
        }
        assert!(started.elapsed() < POLL_BUDGET * 20 + Duration::from_millis(500));
    }
}
