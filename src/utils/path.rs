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

/// Shows `path` with the home directory abbreviated to `~`.
pub fn display_home(path: &Path) -> String {
    display_home_in(path, home().as_deref())
}

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
