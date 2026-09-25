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
    /// Starter code for a new file: the smallest program that runs and prints.
    /// Indented the way the editor indents this language.
    pub template: &'static str,
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
        template: "fn main() {\n    println!(\"Hello, world!\");\n}\n",
        binaries: &["rustc"],
        command: |binary, file| compile_and_run(binary, file, "\"$0\" \"$1\" -o \"$1.out\""),
    },
    Runner {
        name: "Python",
        syntax: "python",
        extension: "py",
        template: "print(\"Hello, world!\")\n",
        binaries: &["python3", "python"],
        command: interpret,
    },
    Runner {
        name: "JavaScript",
        syntax: "javascript",
        extension: "js",
        template: "console.log(\"Hello, world!\");\n",
        binaries: &["node", "bun", "deno"],
        command: interpret,
    },
    Runner {
        name: "TypeScript",
        syntax: "typescript",
        extension: "ts",
        template: "const greeting: string = \"Hello, world!\";\nconsole.log(greeting);\n",
        binaries: &["bun", "deno", "tsx"],
        command: interpret,
    },
    Runner {
        name: "Go",
        syntax: "go",
        extension: "go",
        template: "package main\n\nimport \"fmt\"\n\nfunc main() {\n\tfmt.Println(\"Hello, world!\")\n}\n",
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
        template: "public class Main {\n  public static void main(String[] args) {\n    System.out.println(\"Hello, world!\");\n  }\n}\n",
        binaries: &["java"],
        command: interpret,
    },
    Runner {
        name: "C",
        syntax: "c",
        extension: "c",
        template: "#include <stdio.h>\n\nint main(void) {\n    printf(\"Hello, world!\\n\");\n    return 0;\n}\n",
        binaries: &["cc", "clang", "gcc"],
        command: |binary, file| compile_and_run(binary, file, "\"$0\" -x c \"$1\" -o \"$1.out\""),
    },
    Runner {
        name: "C++",
        syntax: "cpp",
        extension: "cpp",
        template: "#include <iostream>\n\nint main() {\n    std::cout << \"Hello, world!\" << std::endl;\n    return 0;\n}\n",
        binaries: &["c++", "clang++", "g++"],
        command: |binary, file| compile_and_run(binary, file, "\"$0\" -x c++ \"$1\" -o \"$1.out\""),
    },
    Runner {
        name: "Shell",
        syntax: "shell",
        extension: "sh",
        template: "echo \"Hello, world!\"\n",
        binaries: &["bash", "sh"],
        command: interpret,
    },
];

/// Other extensions for a runner's language, as `(extension, runner extension)`.
const EXTENSION_ALIASES: &[(&str, &str)] = &[
    ("pyw", "py"),
    ("mjs", "js"),
    ("cjs", "js"),
    ("mts", "ts"),
    ("h", "c"),
    ("cc", "cpp"),
    ("cxx", "cpp"),
    ("hh", "cpp"),
    ("hpp", "cpp"),
    ("bash", "sh"),
];

/// The runner for a file, going by its extension.
pub fn for_path(path: &Path) -> Option<&'static Runner> {
    let extension = path.extension()?.to_str()?.to_lowercase();
    let extension = EXTENSION_ALIASES
        .iter()
        .find(|(alias, _)| *alias == extension)
        .map_or(extension.as_str(), |(_, target)| target);

    RUNNERS.iter().find(|runner| runner.extension == extension)
}

/// `binary file`
fn interpret(binary: &Path, file: &Path) -> Command {
    let mut command = Command::new(binary);
    command.arg(file);
    command
}

/// Runs `compile` through `sh` (with `$0` = binary, `$1` = file), then replaces the shell
/// with `$1.out` so killing the run kills the program. Arguments added to the
/// command after this are passed on to the program.
fn compile_and_run(binary: &Path, file: &Path, compile: &str) -> Command {
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg(format!(
            "{compile} && out=\"$1.out\" && shift && exec \"$out\" \"$@\""
        ))
        .arg(binary)
        .arg(file);
    command
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn names_and_extensions_are_unique() {
        let names: HashSet<_> = RUNNERS.iter().map(|runner| runner.name).collect();
        let extensions: HashSet<_> = RUNNERS.iter().map(|runner| runner.extension).collect();
        assert_eq!(names.len(), RUNNERS.len());
        assert_eq!(extensions.len(), RUNNERS.len());
    }

    #[test]
    fn every_runner_has_a_binary_and_a_greeting_template() {
        for runner in RUNNERS {
            assert!(
                !runner.binaries.is_empty(),
                "{} has no binaries",
                runner.name
            );
            assert!(
                runner.template.contains("Hello, world!"),
                "{} template doesn't greet",
                runner.name
            );
            assert!(runner.template.ends_with('\n'), "{} template", runner.name);
        }
    }

    #[test]
    fn compiled_languages_run_the_output_through_exec() {
        let command = compile_and_run(Path::new("/bin/cc"), Path::new("/tmp/main.c"), "cc");
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_string_lossy())
            .collect();
        assert_eq!(args[0], "-c");
        assert!(args[1].ends_with("exec \"$out\" \"$@\""));
        // The paths are passed as arguments, never spliced into the script.
        assert_eq!(args[2], "/bin/cc");
        assert_eq!(args[3], "/tmp/main.c");
    }

    #[cfg(unix)]
    #[test]
    fn compiled_programs_receive_arguments() {
        use std::{env, fs, process::Stdio};

        let dir = env::temp_dir().join(format!("scratch-test-args-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("main.sh");
        // A stand-in "compiler" that copies the source to the output path.
        fs::write(&file, "#!/bin/sh\nfor arg; do echo \"[$arg]\"; done\n").unwrap();

        let mut command = compile_and_run(
            Path::new("/bin/cp"),
            &file,
            "\"$0\" \"$1\" \"$1.out\" && chmod +x \"$1.out\"",
        );
        command.args(["one", "two words", ""]);
        let output = command.stderr(Stdio::inherit()).output().unwrap();
        let _ = fs::remove_dir_all(&dir);

        assert!(output.status.success());
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "[one]\n[two words]\n[]\n"
        );
    }

    #[test]
    fn finds_runners_by_extension_and_alias() {
        let name = |path: &str| for_path(Path::new(path)).map(|runner| runner.name);

        assert_eq!(name("a/main.rs"), Some("Rust"));
        assert_eq!(name("x.PY"), Some("Python"));
        assert_eq!(name("lib.hpp"), Some("C++"));
        assert_eq!(name("util.h"), Some("C"));
        assert_eq!(name("app.mjs"), Some("JavaScript"));
        assert_eq!(name("notes.txt"), None);
        assert_eq!(name("Makefile"), None);
    }
}
