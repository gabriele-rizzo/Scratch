use std::{
    env,
    path::{Path, PathBuf},
};

fn home() -> Option<PathBuf> {
    env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

/// Expands a leading `~` to the home directory, like a shell.
pub fn expand_home(path: &str) -> PathBuf {
    expand_home_in(path, home().as_deref())
}

fn expand_home_in(path: &str, home: Option<&Path>) -> PathBuf {
    match (path.strip_prefix('~'), home) {
        (Some(""), Some(home)) => home.to_path_buf(),
        (Some(rest), Some(home)) if rest.starts_with('/') => home.join(&rest[1..]),
        _ => PathBuf::from(path),
    }
}

/// Shows `path` for people: relative to the current folder when it's inside it,
/// otherwise with the home directory abbreviated to `~`.
pub fn display_path(path: &Path) -> String {
    display_path_in(path, env::current_dir().ok().as_deref(), home().as_deref())
}

fn display_path_in(path: &Path, current: Option<&Path>, home: Option<&Path>) -> String {
    match current.and_then(|current| path.strip_prefix(current).ok()) {
        Some(rest) if !rest.as_os_str().is_empty() => rest.display().to_string(),
        _ => display_home_in(path, home),
    }
}

/// Shortens `text` to at most `width` characters by cutting from the start, e.g.
/// `…/proj/main.rs`, so the end of a path stays visible.
pub fn truncate_start(text: &str, width: usize) -> String {
    let count = text.chars().count();
    if count <= width {
        return text.to_string();
    }
    if width == 0 {
        return String::new();
    }

    let tail: String = text.chars().skip(count - (width - 1)).collect();
    format!("…{tail}")
}

/// Shows `path` with the home directory abbreviated to `~`.
fn display_home_in(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "/home/me";

    #[test]
    fn expands_a_leading_tilde() {
        let home = Some(Path::new(HOME));
        assert_eq!(expand_home_in("~", home), Path::new(HOME));
        assert_eq!(
            expand_home_in("~/a/b.rs", home),
            Path::new("/home/me/a/b.rs")
        );
    }

    #[test]
    fn leaves_other_paths_alone() {
        let home = Some(Path::new(HOME));
        assert_eq!(expand_home_in("a/~/b", home), Path::new("a/~/b"));
        // `~user` isn't supported, so it stays a plain name.
        assert_eq!(expand_home_in("~bob/x", home), Path::new("~bob/x"));
        assert_eq!(expand_home_in("~/x", None), Path::new("~/x"));
    }

    #[test]
    fn shows_paths_relative_to_the_current_folder() {
        let home = Some(Path::new(HOME));
        let current = Some(Path::new("/home/me/proj"));

        assert_eq!(
            display_path_in(Path::new("/home/me/proj/src/a.rs"), current, home),
            "src/a.rs"
        );
        assert_eq!(
            display_path_in(Path::new("/home/me/other.rs"), current, home),
            "~/other.rs"
        );
        assert_eq!(
            display_path_in(Path::new("/home/me/proj"), current, home),
            "~/proj"
        );
        assert_eq!(
            display_path_in(Path::new("scratch.rs"), current, home),
            "scratch.rs"
        );
    }

    #[test]
    fn truncates_from_the_start() {
        assert_eq!(truncate_start("proj/main.rs", 20), "proj/main.rs");
        assert_eq!(truncate_start("a/long/path/main.rs", 10), "…h/main.rs");
        assert_eq!(truncate_start("abc", 1), "…");
        assert_eq!(truncate_start("abc", 0), "");
    }

    #[test]
    fn abbreviates_the_home_directory() {
        let home = Some(Path::new(HOME));
        assert_eq!(display_home_in(Path::new("/home/me/a.rs"), home), "~/a.rs");
        assert_eq!(display_home_in(Path::new("/home/me"), home), "~");
        assert_eq!(
            display_home_in(Path::new("/home/meg/a.rs"), home),
            "/home/meg/a.rs"
        );
        assert_eq!(display_home_in(Path::new("rel/a.rs"), home), "rel/a.rs");
    }
}
