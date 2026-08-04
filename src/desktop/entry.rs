use std::{collections::BTreeMap, ffi::OsString, path::PathBuf};

use super::DiscoveryFailure;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ParsedDesktopExec {
    pub(super) executable: PathBuf,
    pub(super) fixed_args: Vec<OsString>,
    pub(super) environment: BTreeMap<OsString, OsString>,
}

pub(super) fn parse_desktop_entry(bytes: &[u8]) -> Result<ParsedDesktopExec, DiscoveryFailure> {
    let input = std::str::from_utf8(bytes)
        .map_err(|_| DiscoveryFailure::unavailable("the Desktop entry is not UTF-8"))?;
    let mut in_main_entry = false;
    let mut found_main_entry = false;
    let mut values = BTreeMap::<String, String>::new();
    for raw_line in input.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            in_main_entry = line == "[Desktop Entry]";
            found_main_entry |= in_main_entry;
            continue;
        }
        if !in_main_entry {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| DiscoveryFailure::unavailable("the main Desktop entry is malformed"))?;
        let key = key.trim();
        let value = value.trim();
        if !valid_desktop_key(key) {
            return Err(DiscoveryFailure::unavailable(
                "the main Desktop entry has an invalid key",
            ));
        }
        if matches!(key, "Type" | "Exec" | "Hidden")
            && values.insert(key.to_owned(), value.to_owned()).is_some()
        {
            return Err(DiscoveryFailure::unavailable(
                "the main Desktop entry has duplicate critical keys",
            ));
        }
    }
    if !found_main_entry {
        return Err(DiscoveryFailure::unavailable(
            "the main Desktop entry is missing",
        ));
    }
    if values.get("Hidden").is_some_and(|value| value == "true") {
        return Err(DiscoveryFailure::unavailable(
            "codex-desktop.desktop is hidden",
        ));
    }
    if values.get("Type").map(String::as_str) != Some("Application") {
        return Err(DiscoveryFailure::unavailable(
            "the Desktop entry is not a launchable application",
        ));
    }
    let exec = values
        .get("Exec")
        .ok_or_else(|| DiscoveryFailure::unavailable("the Desktop entry has no Exec command"))?;
    parse_desktop_exec(exec)
}

fn valid_desktop_key(key: &str) -> bool {
    let (base, locale) = match key.split_once('[') {
        Some((base, locale)) => match locale.strip_suffix(']') {
            Some(locale) if !locale.contains('[') => (base, Some(locale)),
            _ => return false,
        },
        None => (key, None),
    };
    !base.is_empty()
        && base
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '-')
        && locale.is_none_or(|locale| {
            !locale.is_empty()
                && locale.chars().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.' | '@')
                })
        })
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DesktopStringCharacter {
    value: char,
    escaped_backslash: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ExecCharacter {
    value: char,
    quoted: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ExecToken {
    characters: Vec<ExecCharacter>,
    has_unquoted_reserved_character: bool,
    has_unquoted_non_equals_reserved_character: bool,
}

impl ExecToken {
    fn value(&self) -> String {
        self.characters
            .iter()
            .map(|character| character.value)
            .collect()
    }
}

pub(super) fn parse_desktop_exec(input: &str) -> Result<ParsedDesktopExec, DiscoveryFailure> {
    let tokens = tokenize_desktop_exec(input)?;
    let tokens = tokens
        .into_iter()
        .map(process_field_codes)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    let (assignments, executable_index) =
        if tokens.first().is_some_and(|token| token.value() == "env") {
            let mut assignments = BTreeMap::new();
            let mut index = 1;
            while let Some(token) = tokens.get(index) {
                let value = token.value();
                if value.starts_with('-') {
                    return Err(DiscoveryFailure::unavailable(
                        "Desktop Exec uses unsupported env options",
                    ));
                }
                if token.has_unquoted_non_equals_reserved_character {
                    return Err(DiscoveryFailure::unavailable(
                        "Desktop Exec uses shell syntax",
                    ));
                }
                if let Some((key, value)) = parse_environment_assignment(&value) {
                    assignments.insert(OsString::from(key), OsString::from(value));
                    index += 1;
                    continue;
                }
                break;
            }
            (assignments, index)
        } else {
            (BTreeMap::new(), 0)
        };
    let executable = tokens
        .get(executable_index)
        .ok_or_else(|| DiscoveryFailure::unavailable("Desktop Exec has no launcher executable"))?;
    let executable_value = executable.value();
    if executable_value.contains('=') || executable_value == "env" {
        return Err(DiscoveryFailure::unavailable(
            "Desktop Exec uses a shell-style assignment",
        ));
    }
    if executable.has_unquoted_reserved_character {
        return Err(DiscoveryFailure::unavailable(
            "Desktop Exec uses shell syntax",
        ));
    }
    let fixed_args = tokens[executable_index + 1..]
        .iter()
        .map(|token| {
            if token.has_unquoted_reserved_character {
                Err(DiscoveryFailure::unavailable(
                    "Desktop Exec uses shell syntax",
                ))
            } else {
                Ok(OsString::from(token.value()))
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ParsedDesktopExec {
        executable: PathBuf::from(executable.value()),
        fixed_args,
        environment: assignments,
    })
}

fn tokenize_desktop_exec(input: &str) -> Result<Vec<ExecToken>, DiscoveryFailure> {
    let input = unescape_desktop_string_value(input);
    let mut tokens = Vec::new();
    let mut current = Vec::new();
    let mut quoted = false;
    let mut quote_closed = false;
    let mut started = false;
    let mut has_unquoted_reserved_character = false;
    let mut has_unquoted_non_equals_reserved_character = false;
    let mut characters = input.into_iter();
    while let Some(character) = characters.next() {
        if quoted {
            match character.value {
                '"' => {
                    quoted = false;
                    quote_closed = true;
                }
                '\\' => {
                    if !character.escaped_backslash {
                        return Err(DiscoveryFailure::unavailable(
                            "Desktop Exec has an unsupported quoted escape",
                        ));
                    }
                    match characters.next() {
                        Some(
                            escaped @ DesktopStringCharacter {
                                value: '"' | '$' | '`',
                                ..
                            },
                        ) => current.push(ExecCharacter {
                            value: escaped.value,
                            quoted: true,
                        }),
                        Some(
                            escaped @ DesktopStringCharacter {
                                value: '\\',
                                escaped_backslash: true,
                            },
                        ) => current.push(ExecCharacter {
                            value: escaped.value,
                            quoted: true,
                        }),
                        Some(_) | None => {
                            return Err(DiscoveryFailure::unavailable(
                                "Desktop Exec has an unsupported quoted escape",
                            ));
                        }
                    }
                }
                '$' | '`' => {
                    return Err(DiscoveryFailure::unavailable(
                        "Desktop Exec has an unescaped quoted reserved character",
                    ));
                }
                _ => current.push(ExecCharacter {
                    value: character.value,
                    quoted: true,
                }),
            }
            continue;
        }
        match character.value {
            '"' => {
                if started {
                    return Err(DiscoveryFailure::unavailable(
                        "Desktop Exec quotes must enclose a whole argument",
                    ));
                }
                quoted = true;
                started = true;
            }
            '\\' => match characters.next() {
                Some(_) | None => {
                    return Err(DiscoveryFailure::unavailable(
                        "Desktop Exec has an unsupported unquoted escape",
                    ));
                }
            },
            character if character.is_ascii_whitespace() => {
                if started {
                    tokens.push(ExecToken {
                        characters: std::mem::take(&mut current),
                        has_unquoted_reserved_character,
                        has_unquoted_non_equals_reserved_character,
                    });
                    started = false;
                    quote_closed = false;
                    has_unquoted_reserved_character = false;
                    has_unquoted_non_equals_reserved_character = false;
                }
            }
            _ => {
                if quote_closed {
                    return Err(DiscoveryFailure::unavailable(
                        "Desktop Exec quotes must enclose a whole argument",
                    ));
                }
                started = true;
                has_unquoted_reserved_character |= is_exec_reserved_character(character.value);
                has_unquoted_non_equals_reserved_character |=
                    is_exec_reserved_character(character.value) && character.value != '=';
                current.push(ExecCharacter {
                    value: character.value,
                    quoted: false,
                });
            }
        }
    }
    if quoted {
        return Err(DiscoveryFailure::unavailable(
            "Desktop Exec has an unterminated quote",
        ));
    }
    if started {
        tokens.push(ExecToken {
            characters: current,
            has_unquoted_reserved_character,
            has_unquoted_non_equals_reserved_character,
        });
    }
    if tokens.is_empty() {
        return Err(DiscoveryFailure::unavailable("Desktop Exec is empty"));
    }
    Ok(tokens)
}

fn unescape_desktop_string_value(input: &str) -> Vec<DesktopStringCharacter> {
    let mut result = Vec::new();
    let mut characters = input.chars().peekable();
    while let Some(character) = characters.next() {
        if character != '\\' {
            result.push(DesktopStringCharacter {
                value: character,
                escaped_backslash: false,
            });
            continue;
        }
        match characters.peek().copied() {
            Some('s') => {
                characters.next();
                result.push(DesktopStringCharacter {
                    value: ' ',
                    escaped_backslash: false,
                });
            }
            Some('n') => {
                characters.next();
                result.push(DesktopStringCharacter {
                    value: '\n',
                    escaped_backslash: false,
                });
            }
            Some('t') => {
                characters.next();
                result.push(DesktopStringCharacter {
                    value: '\t',
                    escaped_backslash: false,
                });
            }
            Some('r') => {
                characters.next();
                result.push(DesktopStringCharacter {
                    value: '\r',
                    escaped_backslash: false,
                });
            }
            Some('\\') => {
                characters.next();
                result.push(DesktopStringCharacter {
                    value: '\\',
                    escaped_backslash: true,
                });
            }
            Some(_) | None => result.push(DesktopStringCharacter {
                value: '\\',
                escaped_backslash: false,
            }),
        }
    }
    result
}

fn process_field_codes(token: ExecToken) -> Result<Option<ExecToken>, DiscoveryFailure> {
    const STANDARD_OPERANDS: [char; 13] = [
        'f', 'F', 'u', 'U', 'i', 'c', 'k', 'd', 'D', 'n', 'N', 'v', 'm',
    ];
    if let [percent, code] = token.characters.as_slice()
        && percent.value == '%'
        && STANDARD_OPERANDS.contains(&code.value)
    {
        if percent.quoted || code.quoted {
            return Err(DiscoveryFailure::unavailable(
                "Desktop Exec uses a field code inside a quoted argument",
            ));
        }
        return Ok(None);
    }
    let mut result = Vec::new();
    let mut characters = token.characters.into_iter();
    while let Some(character) = characters.next() {
        if character.value != '%' {
            result.push(character);
            continue;
        }
        let code = characters.next().ok_or_else(|| {
            DiscoveryFailure::unavailable("Desktop Exec has an incomplete field code")
        })?;
        if code.value.is_ascii_alphabetic() && (character.quoted || code.quoted) {
            return Err(DiscoveryFailure::unavailable(
                "Desktop Exec uses a field code inside a quoted argument",
            ));
        }
        if code.value == '%' {
            result.push(ExecCharacter {
                value: '%',
                quoted: character.quoted || code.quoted,
            });
        } else if STANDARD_OPERANDS.contains(&code.value) {
            return Err(DiscoveryFailure::unavailable(
                "Desktop Exec has an ambiguous in-argument field code",
            ));
        } else {
            return Err(DiscoveryFailure::unavailable(
                "Desktop Exec has an unsupported field code",
            ));
        }
    }
    Ok(Some(ExecToken {
        characters: result,
        has_unquoted_reserved_character: token.has_unquoted_reserved_character,
        has_unquoted_non_equals_reserved_character: token
            .has_unquoted_non_equals_reserved_character,
    }))
}

fn parse_environment_assignment(token: &str) -> Option<(&str, &str)> {
    let (key, value) = token.split_once('=')?;
    (!key.is_empty()
        && key.chars().enumerate().all(|(index, character)| {
            character == '_'
                || character.is_ascii_alphanumeric()
                    && (index > 0 || character.is_ascii_alphabetic())
        }))
    .then_some((key, value))
}

fn is_exec_reserved_character(character: char) -> bool {
    matches!(
        character,
        '\'' | '>' | '<' | '~' | '|' | '&' | ';' | '$' | '*' | '?' | '#' | '(' | ')' | '`' | '='
    )
}
