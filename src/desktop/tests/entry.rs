use std::ffi::{OsStr, OsString};

use super::super::{
    DiscoveryFailure,
    entry::{parse_desktop_entry, parse_desktop_exec},
};

#[test]
fn desktop_exec_tokenization_rejects_shell_ambiguity_and_preserves_env() {
    let command = parse_desktop_exec(
        r#"env HOME=/child XDG_CONFIG_HOME=/child/config /bin/launcher --flag "two words" %% %U"#,
    )
    .unwrap();
    assert_eq!(
        command.executable,
        std::path::PathBuf::from("/bin/launcher")
    );
    assert_eq!(
        command.fixed_args,
        vec![
            OsString::from("--flag"),
            OsString::from("two words"),
            OsString::from("%")
        ]
    );
    assert_eq!(
        command.environment[&OsString::from("HOME")],
        OsString::from("/child")
    );
    for invalid in [
        "DESKTOP=1 /bin/launcher",
        "env -i /bin/launcher",
        "/bin/launcher --file=%f",
        "/bin/launcher %z",
        "/bin/launcher $HOME",
    ] {
        assert!(
            matches!(
                parse_desktop_exec(invalid),
                Err(DiscoveryFailure::Unavailable(_))
            ),
            "{invalid}"
        );
    }
}

#[test]
fn desktop_entry_requires_one_well_formed_main_application_entry() {
    for entry in [
        b"[Desktop Entry]\nType=Application\nType=Application\nExec=/bin/true\n".as_slice(),
        b"[Desktop Entry]\nType=Application\nExec=/bin/true\nExec=/bin/false\n".as_slice(),
        b"[Desktop Entry]\nType=Link\nExec=/bin/true\n".as_slice(),
        b"[Desktop Entry]\nType=Application\n".as_slice(),
        b"[Other Entry]\nType=Application\nExec=/bin/true\n".as_slice(),
    ] {
        assert!(matches!(
            parse_desktop_entry(entry),
            Err(DiscoveryFailure::Unavailable(_))
        ));
    }
}

#[test]
fn desktop_exec_conforms_to_desktop_entry_two_layer_escaping_and_field_codes() {
    let quoted_backslash = format!(r#"/bin/launcher "{}""#, "\\".repeat(4));
    for (input, expected) in [
        (r#"/bin/launcher """#.to_owned(), vec![OsString::new()]),
        (
            r#"/bin/launcher "a;b""#.to_owned(),
            vec![OsString::from("a;b")],
        ),
        (
            r#"/bin/launcher "a\\"b""#.to_owned(),
            vec![OsString::from("a\"b")],
        ),
        (
            r#"/bin/launcher "\\$HOME""#.to_owned(),
            vec![OsString::from("$HOME")],
        ),
        (
            r#"/bin/launcher "\\`value\\`""#.to_owned(),
            vec![OsString::from("`value`")],
        ),
        (quoted_backslash, vec![OsString::from("\\")]),
        (
            r#"/bin/launcher "\s""#.to_owned(),
            vec![OsString::from(" ")],
        ),
        (
            r#"/bin/launcher "\n""#.to_owned(),
            vec![OsString::from("\n")],
        ),
        (
            r#"/bin/launcher "\t""#.to_owned(),
            vec![OsString::from("\t")],
        ),
        (
            r#"/bin/launcher "\r""#.to_owned(),
            vec![OsString::from("\r")],
        ),
        ("/bin/launcher %%".to_owned(), vec![OsString::from("%")]),
    ] {
        assert_eq!(
            parse_desktop_exec(&input).unwrap().fixed_args,
            expected,
            "{input}"
        );
    }
    for operand in [
        "%f", "%F", "%u", "%U", "%i", "%c", "%k", "%d", "%D", "%n", "%N", "%v", "%m",
    ] {
        assert!(
            parse_desktop_exec(&format!("/bin/launcher {operand}"))
                .unwrap()
                .fixed_args
                .is_empty()
        );
    }
    for invalid in [
        r#"/bin/launcher "unterminated"#,
        r#"/bin/launcher "unsupported\q""#,
        r#"/bin/launcher "unsupported\\q""#,
        r#"/bin/launcher "a\\b""#,
        r#"/bin/launcher "a\"b""#,
        r#"/bin/launcher "\$HOME""#,
        r#"/bin/launcher "\`value\`""#,
        r#"/bin/launcher "$HOME""#,
        r#"/bin/launcher "a`b""#,
        r#"/bin/launcher "a"b"#,
        r#"/bin/launcher `value`"#,
        r#"/bin/launcher "%U""#,
        "/bin/launcher prefix%f",
        "/bin/launcher %Z",
        "/bin/launcher ;",
        "/bin/launcher key=value",
    ] {
        assert!(matches!(
            parse_desktop_exec(invalid),
            Err(DiscoveryFailure::Unavailable(_))
        ));
    }
    let parsed = parse_desktop_exec("env HOME=/tmp /bin/launcher").unwrap();
    assert_eq!(
        parsed.environment.get(OsStr::new("HOME")),
        Some(&OsString::from("/tmp"))
    );
    assert!(matches!(
        parse_desktop_exec("env HOME=/tmp key=value"),
        Err(DiscoveryFailure::Unavailable(_))
    ));
}

#[test]
fn desktop_entry_trims_delimiter_whitespace_and_rejects_critical_duplicates() {
    let command = parse_desktop_entry(
        b"[Desktop Entry]\n Type = Application \n Exec = /bin/launcher --flag \n",
    )
    .unwrap();
    assert_eq!(
        command.executable,
        std::path::PathBuf::from("/bin/launcher")
    );
    assert_eq!(command.fixed_args, vec![OsString::from("--flag")]);
    assert!(matches!(
        parse_desktop_entry(
            b"[Desktop Entry]\nType = Application\n Type=Application\nExec=/bin/launcher\n"
        ),
        Err(DiscoveryFailure::Unavailable(_))
    ));
}
