use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
    thread,
};

use super::RUNNERS;

pub enum Detection {
    Found(PathBuf),
    Missing,
    /// Only a placeholder was found (like macOS's `/usr/bin/java` without a JDK). The
    /// text says what to install.
    Placeholder(&'static str),
}

/// Checks every runner on a single background thread and streams `(index, detection)`
/// results as they complete. Detection only stats files (no processes are spawned),
/// so the whole scan is a handful of syscalls.
pub fn detect() -> Receiver<(usize, Detection)> {
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let path = env::var_os("PATH").unwrap_or_default();
        let dirs: Vec<PathBuf> = env::split_paths(&path).collect();

        for (index, runner) in RUNNERS.iter().enumerate() {
            let detection = detect_runner(&dirs, runner.binaries);

            if tx.send((index, detection)).is_err() {
                break;
            }
        }
    });

    rx
}

fn detect_runner(dirs: &[PathBuf], binaries: &[&str]) -> Detection {
    let mut placeholder = None;

    for binary in binaries {
        for path in dirs.iter().map(|dir| dir.join(binary)) {
            if !is_executable(&path) {
                continue;
            }

            match placeholder_hint(&path) {
                // Keep looking: a real install may come later in PATH.
                Some(hint) => placeholder = placeholder.or(Some(hint)),
                None => return Detection::Found(path),
            }
        }
    }

    placeholder.map_or(Detection::Missing, Detection::Placeholder)
}

fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
    }

    #[cfg(not(unix))]
    metadata.is_file()
}

/// If `path` is a placeholder that can't actually run anything, says what's missing.
#[cfg(target_os = "macos")]
fn placeholder_hint(path: &Path) -> Option<&'static str> {
    macos::placeholder_hint(path)
}

#[cfg(not(target_os = "macos"))]
fn placeholder_hint(_path: &Path) -> Option<&'static str> {
    None
}

/// macOS ships stubs in `/usr/bin` that only forward to real tools, and running one
/// without the tools installed opens an install dialog. So instead of running them,
/// look for what they would forward to.
#[cfg(target_os = "macos")]
mod macos {
    use std::{
        env, fs,
        path::{Path, PathBuf},
    };

    /// Stubs forwarding to the active developer directory (Xcode or the Command Line Tools).
    const DEVELOPER_TOOLS: &[&str] = &["python3", "cc", "c++", "clang", "clang++", "gcc", "g++"];

    pub fn placeholder_hint(path: &Path) -> Option<&'static str> {
        if path.parent() != Some(Path::new("/usr/bin")) {
            return None;
        }

        let name = path.file_name()?.to_str()?;

        if DEVELOPER_TOOLS.contains(&name) {
            return (!has_developer_tool(name))
                .then_some("needs Xcode Command Line Tools (xcode-select --install)");
        }

        if name == "java" {
            return (!has_jdk()).then_some("no JDK installed");
        }

        None
    }

    /// Mirrors how the stubs pick a developer directory: `DEVELOPER_DIR`, then the
    /// `xcode-select` choice, then the Command Line Tools.
    fn developer_dir() -> PathBuf {
        env::var_os("DEVELOPER_DIR")
            .map(PathBuf::from)
            .or_else(|| fs::read_link("/var/db/xcode_select_link").ok())
            .unwrap_or_else(|| PathBuf::from("/Library/Developer/CommandLineTools"))
    }

    fn has_developer_tool(name: &str) -> bool {
        let dir = developer_dir();

        [
            dir.join("usr/bin").join(name),
            dir.join("Toolchains/XcodeDefault.xctoolchain/usr/bin")
                .join(name),
        ]
        .iter()
        .any(|path| path.exists())
    }

    /// `/usr/bin/java` uses `JAVA_HOME`, or any JDK in the standard locations.
    fn has_jdk() -> bool {
        if let Some(home) = env::var_os("JAVA_HOME")
            && Path::new(&home).join("bin/java").exists()
        {
            return true;
        }

        let user = env::var_os("HOME")
            .map(|home| PathBuf::from(home).join("Library/Java/JavaVirtualMachines"));
        let roots = [
            Some(PathBuf::from("/Library/Java/JavaVirtualMachines")),
            user,
        ];

        roots.into_iter().flatten().any(|root| {
            fs::read_dir(root).is_ok_and(|entries| {
                entries
                    .flatten()
                    .any(|entry| entry.path().join("Contents/Home/bin/java").exists())
            })
        })
    }
}
