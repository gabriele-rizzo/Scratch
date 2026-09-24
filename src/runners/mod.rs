use std::{path::Path, process::Command};

mod detect;
pub use detect::*;

mod process;
pub use process::*;

pub struct Runner {
    /// Name shown to the user and matched against the input.
    pub name: &'static str,
    /// Language id understood by the editor's syntax highlighter.
    pub syntax: &'static str,
    /// File extension used when saving and running.
    pub extension: &'static str,
    /// Executables that can run this language, in order of preference.
    pub binaries: &'static [&'static str],
    /// Builds the command that runs `file` with the resolved `binary`.
    pub command: fn(binary: &Path, file: &Path) -> Command,
}

pub const RUNNERS: &[Runner] = &[
    Runner {
        name: "Rust",
        syntax: "rust",
        extension: "rs",
        binaries: &["rustc"],
        command: |binary, file| compile_and_run(binary, file, "\"$0\" \"$1\" -o \"$1.out\""),
    },
    Runner {
        name: "Python",
        syntax: "python",
        extension: "py",
        binaries: &["python3", "python"],
        command: interpret,
    },
    Runner {
        name: "JavaScript",
        syntax: "javascript",
        extension: "js",
        binaries: &["node", "bun", "deno"],
        command: interpret,
    },
    Runner {
        name: "TypeScript",
        syntax: "typescript",
        extension: "ts",
        binaries: &["bun", "deno", "tsx"],
        command: interpret,
    },
    Runner {
        name: "Go",
        syntax: "go",
        extension: "go",
        binaries: &["go"],
        command: |binary, file| {
            let mut command = Command::new(binary);
            command.arg("run").arg(file);
            command
        },
    },
    Runner {
        name: "Java",
        syntax: "java",
        extension: "java",
        binaries: &["java"],
        command: interpret,
    },
    Runner {
        name: "C",
        syntax: "c",
        extension: "c",
        binaries: &["cc", "clang", "gcc"],
        command: |binary, file| compile_and_run(binary, file, "\"$0\" -x c \"$1\" -o \"$1.out\""),
    },
    Runner {
        name: "C++",
        syntax: "cpp",
        extension: "cpp",
        binaries: &["c++", "clang++", "g++"],
        command: |binary, file| compile_and_run(binary, file, "\"$0\" -x c++ \"$1\" -o \"$1.out\""),
    },
    Runner {
        name: "Shell",
        syntax: "shell",
        extension: "sh",
        binaries: &["bash", "sh"],
        command: interpret,
    },
];

/// `binary file`
fn interpret(binary: &Path, file: &Path) -> Command {
    let mut command = Command::new(binary);
    command.arg(file);
    command
}

/// Runs `compile` through `sh` (with `$0` = binary, `$1` = file), then replaces the shell
/// with `$1.out` so killing the run kills the program.
fn compile_and_run(binary: &Path, file: &Path, compile: &str) -> Command {
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg(format!("{compile} && exec \"$1.out\""))
        .arg(binary)
        .arg(file);
    command
}
