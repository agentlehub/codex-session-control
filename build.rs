use std::fs;

const VERSION_FILE: &str = "supported-codex-version.txt";
const VERSION_ENV: &str = "CODEX_SESSION_CONTROL_TESTED_CODEX_VERSION";

fn main() {
    println!("cargo::rerun-if-changed={VERSION_FILE}");
    let raw = fs::read_to_string(VERSION_FILE)
        .unwrap_or_else(|error| panic!("cannot read {VERSION_FILE}: {error}"));
    let version = raw
        .strip_suffix('\n')
        .expect("supported Codex version must end with one newline");
    assert!(
        !version.contains(['\r', '\n']),
        "supported Codex version must contain exactly one line"
    );
    let components = version.split('.').collect::<Vec<_>>();
    assert_eq!(
        components.len(),
        3,
        "supported Codex version must be stable SemVer"
    );
    assert!(
        components.iter().all(|component| {
            !component.is_empty()
                && component.bytes().all(|byte| byte.is_ascii_digit())
                && (component == &"0" || !component.starts_with('0'))
        }),
        "supported Codex version must be stable SemVer"
    );
    println!("cargo::rustc-env={VERSION_ENV}={version}");
}
