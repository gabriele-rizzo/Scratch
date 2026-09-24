use std::{
    io::{BufRead, BufReader, Read},
    process::{Child, Command, ExitStatus, Stdio},
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}

pub enum RunStatus {
    Running,
    Exited(ExitStatus, Duration),
}

/// A running (or finished) program whose output is collected line by line.
pub struct Process {
    child: Child,
    output: Receiver<(Stream, String)>,
    pub lines: Vec<(Stream, String)>,
    pub started: Instant,
    pub status: RunStatus,
}

impl Process {
    pub fn spawn(mut command: Command) -> std::io::Result<Self> {
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let (tx, rx) = mpsc::channel();

        if let Some(stdout) = child.stdout.take() {
            forward(stdout, Stream::Stdout, tx.clone());
        }
        if let Some(stderr) = child.stderr.take() {
            forward(stderr, Stream::Stderr, tx);
        }

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
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn forward(pipe: impl Read + Send + 'static, stream: Stream, tx: Sender<(Stream, String)>) {
    thread::spawn(move || {
        let mut reader = BufReader::new(pipe);
        let mut buffer = Vec::new();

        while let Ok(read) = reader.read_until(b'\n', &mut buffer) {
            if read == 0 {
                break;
            }

            let line = String::from_utf8_lossy(&buffer)
                .trim_end_matches(['\n', '\r'])
                .replace('\t', "    ");

            if tx.send((stream, line)).is_err() {
                break;
            }

            buffer.clear();
        }
    });
}
