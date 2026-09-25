use std::{fs, path::PathBuf};

use super::expand_home;

/// At most this many entries are listed.
const MAX_ENTRIES: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
}

/// Files and folders that could complete a partly typed path.
#[derive(Debug, Default)]
pub struct PathCompletion {
    /// The typed text up to and including the last `/`, kept exactly as typed.
    dir: String,
    /// The typed part of the name after it, unescaped.
    prefix: String,
    pub entries: Vec<Entry>,
}

impl PathCompletion {
    /// Lists entries matching `typed`, which may use `~`, quotes and `\` escapes.
    pub fn new(typed: &str) -> Self {
        let unescaped = unescape(typed);

        // `~` on its own means the home folder.
        if unescaped == "~" {
            return Self {
                dir: String::new(),
                prefix: "~".to_string(),
                entries: vec![Entry {
                    name: "~".to_string(),
                    is_dir: true,
                }],
            };
        }

        let (dir, prefix) = match unescaped.rfind('/') {
            Some(slash) => (&unescaped[..=slash], &unescaped[slash + 1..]),
            None => ("", unescaped.as_str()),
        };
        let folder = if dir.is_empty() {
            PathBuf::from(".")
        } else {
            expand_home(dir)
        };

        let mut entries: Vec<Entry> = fs::read_dir(folder)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().into_string().ok()?;
                // Hidden entries only once a `.` is typed, like a shell.
                let visible = !name.starts_with('.') || prefix.starts_with('.');
                (visible && name.starts_with(prefix)).then(|| Entry {
                    // Follows symlinks, so a link to a folder completes like one.
                    is_dir: entry.path().is_dir(),
                    name,
                })
            })
            .take(MAX_ENTRIES)
            .collect();
        entries.sort_by_key(|entry| entry.name.to_lowercase());

        // Keep the folder part exactly as typed (quotes and escapes included).
        let dir_text = match typed.rfind('/') {
            Some(slash) => typed[..=slash].to_string(),
            None => String::new(),
        };

        Self {
            dir: dir_text,
            prefix: prefix.to_string(),
            entries,
        }
    }

    /// Whether the typed text already names the only match, a file, so there's
    /// nothing left to offer.
    pub fn is_complete(&self) -> bool {
        matches!(self.entries.as_slice(), [only] if !only.is_dir && only.name == self.prefix)
    }

    /// The typed text with `entry` filled in; folders end in `/` to keep going.
    pub fn apply(&self, entry: &Entry) -> String {
        let slash = if entry.is_dir { "/" } else { "" };
        if entry.name == "~" {
            return "~/".to_string();
        }
        format!("{}{}{slash}", self.dir, escape(&entry.name))
    }

    /// The typed text extended by what all entries share, if that adds anything.
    pub fn common(&self) -> Option<String> {
        let first = &self.entries.first()?.name;
        let shared = self
            .entries
            .iter()
            .skip(1)
            .fold(first.as_str(), |shared, entry| {
                let length = shared
                    .char_indices()
                    .zip(entry.name.chars())
                    .take_while(|((_, a), b)| a == b)
                    .last()
                    .map_or(0, |((index, c), _)| index + c.len_utf8());
                &shared[..length]
            });

        (shared.len() > self.prefix.len()).then(|| format!("{}{}", self.dir, escape(shared)))
    }
}

/// Escapes characters `split_args` would treat specially.
fn escape(name: &str) -> String {
    let mut escaped = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_whitespace() || matches!(c, '\'' | '"' | '\\') {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

/// Reads a partly typed argument the way `split_args` would, forgiving an
/// unclosed quote or a trailing backslash.
fn unescape(typed: &str) -> String {
    let mut text = String::with_capacity(typed.len());
    let mut quote = None;
    let mut chars = typed.chars();

    while let Some(c) = chars.next() {
        match (c, quote) {
            ('\'' | '"', None) => quote = Some(c),
            (c, Some(open)) if c == open => quote = None,
            ('\\', Some('\'')) => text.push('\\'),
            ('\\', _) => text.extend(chars.next()),
            (c, _) => text.push(c),
        }
    }

    text
}

#[cfg(test)]
mod tests {
    use std::{env, process};

    use super::*;
    use crate::utils::split_args;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str, files: &[&str]) -> Self {
            let dir = env::temp_dir().join(format!("scratch-test-{name}-{}", process::id()));
            let _ = fs::remove_dir_all(&dir);
            for file in files {
                let path = dir.join(file);
                if let Some(folder) = file.strip_suffix('/') {
                    fs::create_dir_all(dir.join(folder)).unwrap();
                } else {
                    fs::create_dir_all(path.parent().unwrap()).unwrap();
                    fs::write(path, "").unwrap();
                }
            }
            Self(dir)
        }

        fn typed(&self, rest: &str) -> String {
            format!("{}/{rest}", self.0.display())
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn names(completion: &PathCompletion) -> Vec<&str> {
        completion
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect()
    }

    #[test]
    fn lists_matching_entries_in_order() {
        let dir = TempDir::new("complete-list", &["main.rs", "Makefile", "lib.rs", "src/"]);
        let completion = PathCompletion::new(&dir.typed("m"));
        assert_eq!(names(&completion), ["main.rs"]);

        let all = PathCompletion::new(&dir.typed(""));
        assert_eq!(names(&all), ["lib.rs", "main.rs", "Makefile", "src"]);
        assert!(all.entries[3].is_dir);
    }

    #[test]
    fn hidden_entries_need_a_dot() {
        let dir = TempDir::new("complete-hidden", &[".env", "app.py"]);
        assert_eq!(names(&PathCompletion::new(&dir.typed(""))), ["app.py"]);
        assert_eq!(names(&PathCompletion::new(&dir.typed("."))), [".env"]);
    }

    #[test]
    fn applying_fills_in_names_and_adds_a_slash_for_folders() {
        let dir = TempDir::new("complete-apply", &["src/", "main.rs"]);
        let typed = dir.typed("s");
        let completion = PathCompletion::new(&typed);
        assert_eq!(completion.apply(&completion.entries[0]), dir.typed("src/"));
    }

    #[test]
    fn common_extends_to_the_shared_prefix() {
        let dir = TempDir::new(
            "complete-common",
            &["test_one.py", "test_two.py", "other.py"],
        );
        let completion = PathCompletion::new(&dir.typed("t"));
        assert_eq!(completion.common(), Some(dir.typed("test_")));

        // Nothing more to add once the shared part is typed.
        assert_eq!(PathCompletion::new(&dir.typed("test_")).common(), None);
    }

    #[test]
    fn names_with_spaces_are_escaped_and_parse_back() {
        let dir = TempDir::new("complete-spaces", &["my file.py"]);
        let completion = PathCompletion::new(&dir.typed("my"));
        let completed = completion.apply(&completion.entries[0]);

        assert!(completed.ends_with(r"my\ file.py"));
        assert_eq!(split_args(&completed).unwrap(), [dir.typed("my file.py")]);

        // And typing continues to match through the escape.
        assert_eq!(
            names(&PathCompletion::new(&dir.typed(r"my\ f"))),
            ["my file.py"]
        );
    }

    #[test]
    fn a_fully_typed_file_is_complete() {
        let dir = TempDir::new("complete-done", &["main.rs", "src/"]);
        assert!(PathCompletion::new(&dir.typed("main.rs")).is_complete());
        assert!(!PathCompletion::new(&dir.typed("mai")).is_complete());
        assert!(!PathCompletion::new(&dir.typed("src")).is_complete());
    }

    #[test]
    fn a_lone_tilde_completes_to_home() {
        let completion = PathCompletion::new("~");
        assert_eq!(completion.apply(&completion.entries[0]), "~/");
    }

    #[test]
    fn unescape_forgives_unfinished_input() {
        assert_eq!(unescape(r#""my dir/fi"#), "my dir/fi");
        assert_eq!(unescape(r"a\ b\"), "a b");
        assert_eq!(unescape(r"'a\b'"), r"a\b");
    }
}
