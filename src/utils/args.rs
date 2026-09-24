/// Splits `input` into arguments like a shell: whitespace separates them, quotes
/// group them, and a backslash escapes the next character (outside single quotes).
pub fn split_args(input: &str) -> Result<Vec<String>, &'static str> {
    let mut args = Vec::new();
    let mut current = String::new();
    // Whether `current` is an argument, even an empty one like `""`.
    let mut started = false;
    let mut chars = input.chars();

    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                started = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => current.push(c),
                        None => return Err("missing closing '"),
                    }
                }
            }
            '"' => {
                started = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(c @ ('"' | '\\')) => current.push(c),
                            Some(c) => {
                                current.push('\\');
                                current.push(c);
                            }
                            None => return Err("missing closing \""),
                        },
                        Some(c) => current.push(c),
                        None => return Err("missing closing \""),
                    }
                }
            }
            '\\' => {
                started = true;
                match chars.next() {
                    Some(c) => current.push(c),
                    None => return Err("nothing to escape after \\"),
                }
            }
            c if c.is_whitespace() => {
                if started {
                    args.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            c => {
                started = true;
                current.push(c);
            }
        }
    }

    if started {
        args.push(current);
    }

    Ok(args)
}

/// Formats arguments for display, quoting any that need it.
pub fn join_args(args: &[String]) -> String {
    args.iter()
        .map(|arg| {
            let plain = !arg.is_empty()
                && arg
                    .chars()
                    .all(|c| !c.is_whitespace() && !matches!(c, '\'' | '"' | '\\'));
            if plain {
                arg.clone()
            } else {
                format!("'{}'", arg.replace('\'', r"'\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(input: &str) -> Vec<String> {
        split_args(input).unwrap()
    }

    #[test]
    fn splits_on_whitespace() {
        assert_eq!(split("  a b\tc  "), ["a", "b", "c"]);
        assert!(split("   ").is_empty());
    }

    #[test]
    fn quotes_group_words() {
        assert_eq!(
            split(r#"say "hello world" 'it''s'"#),
            ["say", "hello world", "its"]
        );
        assert_eq!(split(r#"--name="Bob Smith""#), ["--name=Bob Smith"]);
    }

    #[test]
    fn keeps_empty_quoted_arguments() {
        assert_eq!(split(r#"a "" ''"#), ["a", "", ""]);
    }

    #[test]
    fn backslash_escapes() {
        assert_eq!(split(r"a\ b c\\d"), ["a b", r"c\d"]);
        assert_eq!(split(r#""say \"hi\"""#), [r#"say "hi""#]);
        // Only quotes and backslashes are special inside double quotes.
        assert_eq!(split(r#""a\nb""#), [r"a\nb"]);
        // Nothing is special inside single quotes.
        assert_eq!(split(r"'a\b'"), [r"a\b"]);
    }

    #[test]
    fn reports_unfinished_input() {
        assert!(split_args("'open").is_err());
        assert!(split_args("\"open").is_err());
        assert!(split_args("trailing\\").is_err());
    }

    #[test]
    fn join_round_trips_through_split() {
        let args: Vec<String> = ["plain", "two words", "", "it's", r#"q"uote"#, r"back\slash"]
            .map(String::from)
            .into();
        assert_eq!(split(&join_args(&args)), args);
        assert_eq!(join_args(&args[..1]), "plain");
    }
}
