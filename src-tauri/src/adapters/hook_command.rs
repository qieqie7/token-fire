use std::path::{Path, PathBuf};

use crate::adapters::source::TokenSourceKind;

pub fn tokenfire_hook_command(hook_path: &Path, source: TokenSourceKind) -> String {
    format!(
        "'{}' --source {} --owner token-fire",
        shell_single_quote_escape(&hook_path.display().to_string()),
        source.as_str()
    )
}

pub fn is_tokenfire_owned_command_for_source(command: &str, source: TokenSourceKind) -> bool {
    let Some(args) = shell_words(command) else {
        return false;
    };
    let Some(hook_path) = hook_path_from_single_quoted_command(command) else {
        return false;
    };
    if args
        .first()
        .is_none_or(|first| Path::new(first) != hook_path.as_path())
    {
        return false;
    }
    if hook_path.file_name().and_then(|name| name.to_str()) != Some("token-fire-hook") {
        return false;
    }
    has_arg_pair(&args, "--owner", "token-fire") && has_arg_pair(&args, "--source", source.as_str())
}

pub fn hook_path_from_single_quoted_command(command: &str) -> Option<PathBuf> {
    if !command.trim_start().starts_with('\'') {
        return None;
    }
    let args = shell_words(command)?;
    args.first().map(PathBuf::from)
}

pub fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn shell_single_quote_escape(value: &str) -> String {
    value.replace('\'', "'\"'\"'")
}

fn has_arg_pair(args: &[String], name: &str, value: &str) -> bool {
    args.windows(2)
        .any(|pair| pair[0] == name && pair[1] == value)
}

fn shell_words(command: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut chars = command.chars();
    let mut in_word = false;
    let mut quote = Quote::None;

    while let Some(ch) = chars.next() {
        match quote {
            Quote::None => match ch {
                '\'' => {
                    in_word = true;
                    quote = Quote::Single;
                }
                '"' => {
                    in_word = true;
                    quote = Quote::Double;
                }
                '\\' => {
                    in_word = true;
                    current.push(chars.next()?);
                }
                ch if ch.is_whitespace() => {
                    if in_word {
                        words.push(std::mem::take(&mut current));
                        in_word = false;
                    }
                }
                ch => {
                    in_word = true;
                    current.push(ch);
                }
            },
            Quote::Single => match ch {
                '\'' => quote = Quote::None,
                ch => current.push(ch),
            },
            Quote::Double => match ch {
                '"' => quote = Quote::None,
                '\\' => current.push(chars.next()?),
                ch => current.push(ch),
            },
        }
    }

    if quote != Quote::None {
        return None;
    }
    if in_word {
        words.push(current);
    }
    Some(words)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Quote {
    None,
    Single,
    Double,
}
