use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
    thread,
};

use super::RUNNERS;

/// Checks every runner on a single background thread and streams `(index, binary)`
/// results as they complete. Detection only stats files in `PATH` (no processes are
/// spawned), so the whole scan is a handful of syscalls.
pub fn detect() -> Receiver<(usize, Option<PathBuf>)> {
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let path = env::var_os("PATH").unwrap_or_default();
        let dirs: Vec<PathBuf> = env::split_paths(&path).collect();

        for (index, runner) in RUNNERS.iter().enumerate() {
            let binary = runner
                .binaries
                .iter()
                .find_map(|binary| find_in(&dirs, binary));

            if tx.send((index, binary)).is_err() {
                break;
            }
        }
    });

    rx
}

fn find_in(dirs: &[PathBuf], binary: &str) -> Option<PathBuf> {
    dirs.iter()
        .map(|dir| dir.join(binary))
        .find(|path| is_executable(path))
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
