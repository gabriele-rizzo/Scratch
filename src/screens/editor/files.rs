use std::{
    fs, io,
    path::{Path, PathBuf},
};

/// Files larger than this aren't opened; the editor is meant for scratch code.
const MAX_OPEN_BYTES: u64 = 5 * 1024 * 1024;

/// Reads a text file for editing, explaining in plain words why it can't be.
pub fn read_text(path: &Path) -> Result<String, String> {
    let metadata = fs::metadata(path).map_err(|err| describe_io_error(&err))?;

    if metadata.is_dir() {
        return Err("that's a folder".to_string());
    }
    if metadata.len() > MAX_OPEN_BYTES {
        let megabytes = metadata.len().div_ceil(1024 * 1024);
        return Err(format!("it's too large to edit here ({megabytes} MB)"));
    }

    let bytes = fs::read(path).map_err(|err| describe_io_error(&err))?;
    String::from_utf8(bytes).map_err(|_| "it isn't a text file".to_string())
}

/// Writes `content` to `path`, creating missing parent folders. Returns the
/// topmost folder it had to create, if any.
pub fn write_creating_dirs(path: &Path, content: &str) -> io::Result<Option<PathBuf>> {
    let mut created = None;

    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        // The topmost missing ancestor is what gets created.
        created = parent
            .ancestors()
            .take_while(|dir| !dir.as_os_str().is_empty() && !dir.exists())
            .last()
            .map(Path::to_path_buf);
        fs::create_dir_all(parent)?;
    }

    fs::write(path, content)?;
    Ok(created)
}

/// Writes through a temporary file and renames it into place, so a crash never
/// leaves a half-written file behind.
pub fn write_atomic(path: &Path, content: &str) -> io::Result<()> {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");

    fs::write(&temporary, content)?;
    fs::rename(&temporary, path)
}

/// Explains an I/O error in plain words, without the OS error number.
pub fn describe_io_error(err: &io::Error) -> String {
    match err.kind() {
        io::ErrorKind::NotFound => "no such file or folder".to_string(),
        io::ErrorKind::PermissionDenied => "permission denied".to_string(),
        io::ErrorKind::IsADirectory => "that's a folder".to_string(),
        io::ErrorKind::NotADirectory => "part of the path is a file, not a folder".to_string(),
        io::ErrorKind::ReadOnlyFilesystem => "the disk is read-only".to_string(),
        io::ErrorKind::StorageFull => "the disk is full".to_string(),
        _ => {
            let message = err.to_string();
            match message.find(" (os error") {
                Some(end) => message[..end].to_string(),
                None => message,
            }
        }
    }
}

#[cfg(test)]
pub mod tests {
    use std::{env, process};

    use super::*;

    /// A fresh directory under the system temp dir, removed on drop.
    pub struct TempDir(pub PathBuf);

    impl TempDir {
        pub fn new(name: &str) -> Self {
            let dir = env::temp_dir().join(format!("scratch-test-{name}-{}", process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn saving_creates_missing_folders_and_reports_the_topmost() {
        let dir = TempDir::new("save-dirs");
        let path = dir.0.join("a/b/c.py");

        let created = write_creating_dirs(&path, "x").unwrap();
        assert_eq!(created, Some(dir.0.join("a")));
        assert_eq!(fs::read_to_string(&path).unwrap(), "x");

        // Nothing new to create the second time.
        assert_eq!(write_creating_dirs(&path, "y").unwrap(), None);
    }

    #[test]
    fn describes_common_errors_plainly() {
        let dir = TempDir::new("save-errors");
        let file = dir.0.join("file");
        fs::write(&file, "").unwrap();

        let under_file = write_creating_dirs(&file.join("x.py"), "").unwrap_err();
        assert!(!describe_io_error(&under_file).contains("os error"));

        let onto_dir = fs::write(&dir.0, "").unwrap_err();
        assert_eq!(describe_io_error(&onto_dir), "that's a folder");
    }

    #[test]
    fn reads_text_files_and_explains_the_rest() {
        let dir = TempDir::new("read-text");
        let text = dir.0.join("a.py");
        let binary = dir.0.join("a.bin");
        fs::write(&text, "print(1)\n").unwrap();
        fs::write(&binary, [0xff, 0xfe, 0x00]).unwrap();

        assert_eq!(read_text(&text).unwrap(), "print(1)\n");
        assert_eq!(read_text(&binary).unwrap_err(), "it isn't a text file");
        assert_eq!(read_text(&dir.0).unwrap_err(), "that's a folder");
        assert_eq!(
            read_text(&dir.0.join("missing.py")).unwrap_err(),
            "no such file or folder"
        );
    }

    #[test]
    fn refuses_huge_files() {
        let dir = TempDir::new("read-huge");
        let huge = dir.0.join("huge.txt");
        let file = fs::File::create(&huge).unwrap();
        file.set_len(MAX_OPEN_BYTES + 1).unwrap();

        assert!(read_text(&huge).unwrap_err().contains("too large"));
    }

    #[test]
    fn atomic_writes_replace_the_file_and_leave_no_temporary() {
        let dir = TempDir::new("atomic");
        let path = dir.0.join("draft.py");

        write_atomic(&path, "one").unwrap();
        write_atomic(&path, "two").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "two");
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 1);
    }
}
