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
    let parsed = semver::Version::parse(version).unwrap_or_else(|error| {
        panic!("supported Codex version must be canonical SemVer: {error}")
    });
    assert!(
        parsed.to_string() == version,
        "supported Codex version must be canonical SemVer"
    );
    println!("cargo::rustc-env={VERSION_ENV}={version}");
}
