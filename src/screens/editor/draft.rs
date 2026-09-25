//! Keeps each language's last buffer between sessions.

use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

use super::files::write_atomic;
use crate::runners::Runner;

/// A language's last buffer: its text, cursor, and the file it belongs to.
#[derive(Debug, PartialEq, Eq)]
pub struct Draft {
    pub content: String,
    pub cursor: usize,
    pub path: Option<PathBuf>,
}

impl Draft {
    pub fn load(runner: &Runner) -> Option<Self> {
        Self::load_from(&dir()?, runner.extension)
    }

    pub fn store(&self, runner: &Runner) -> io::Result<()> {
        match dir() {
            Some(dir) => self.store_in(&dir, runner.extension),
            None => Ok(()),
        }
    }

    fn load_from(dir: &Path, extension: &str) -> Option<Self> {
        let content = fs::read_to_string(dir.join(format!("draft.{extension}"))).ok()?;
        let meta =
            fs::read_to_string(dir.join(format!("draft.{extension}.meta"))).unwrap_or_default();

        let mut cursor = 0;
        let mut path = None;

        for line in meta.lines() {
            if let Some(value) = line.strip_prefix("cursor=") {
                cursor = value.parse().unwrap_or(0);
            } else if let Some(value) = line.strip_prefix("path=") {
                path = Some(PathBuf::from(value));
            }
        }

        Some(Self {
            cursor: cursor.min(content.chars().count()),
            content,
            path,
        })
    }

    fn store_in(&self, dir: &Path, extension: &str) -> io::Result<()> {
        fs::create_dir_all(dir)?;

        let mut meta = format!("cursor={}\n", self.cursor);
        if let Some(path) = &self.path {
            meta.push_str(&format!("path={}\n", path.display()));
        }

        write_atomic(&dir.join(format!("draft.{extension}")), &self.content)?;
        write_atomic(&dir.join(format!("draft.{extension}.meta")), &meta)
    }
}

/// Where drafts live: `SCRATCH_DATA_DIR` if set, otherwise the platform's data
/// folder. Tests never touch real drafts.
fn dir() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }

    let nonempty = |name: &str| env::var_os(name).filter(|value| !value.is_empty());

    if let Some(dir) = nonempty("SCRATCH_DATA_DIR") {
        return Some(PathBuf::from(dir).join("drafts"));
    }

    let home = nonempty("HOME").map(PathBuf::from);

    let base = if cfg!(target_os = "macos") {
        home?.join("Library/Application Support")
    } else {
        nonempty("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| home.map(|home| home.join(".local/share")))?
    };

    Some(base.join("scratch/drafts"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::editor::files::tests::TempDir;

    #[test]
    fn round_trips_content_cursor_and_path() {
        let dir = TempDir::new("draft-round-trip");
        let draft = Draft {
            content: "print('hi')\n".to_string(),
            cursor: 5,
            path: Some(PathBuf::from("/home/me/a.py")),
        };

        draft.store_in(&dir.0, "py").unwrap();
        assert_eq!(Draft::load_from(&dir.0, "py"), Some(draft));
    }

    #[test]
    fn drafts_are_kept_per_language() {
        let dir = TempDir::new("draft-languages");
        let python = Draft {
            content: "py".to_string(),
            cursor: 0,
            path: None,
        };

        python.store_in(&dir.0, "py").unwrap();
        assert_eq!(Draft::load_from(&dir.0, "rs"), None);
        assert_eq!(Draft::load_from(&dir.0, "py"), Some(python));
    }

    #[test]
    fn tolerates_missing_or_damaged_metadata() {
        let dir = TempDir::new("draft-meta");
        fs::write(dir.0.join("draft.rs"), "fn main() {}").unwrap();
        assert_eq!(Draft::load_from(&dir.0, "rs").unwrap().cursor, 0);

        // A cursor past the end is clamped.
        fs::write(dir.0.join("draft.rs.meta"), "cursor=9999\nnonsense\n").unwrap();
        let draft = Draft::load_from(&dir.0, "rs").unwrap();
        assert_eq!(draft.cursor, "fn main() {}".len());
        assert_eq!(draft.path, None);
    }

    #[test]
    fn tests_never_use_the_real_data_folder() {
        assert_eq!(dir(), None);
    }
}
