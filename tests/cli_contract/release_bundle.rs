use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

const RELEASE_ASSETS: [&str; 4] = [
    "codex-session-control-x86_64-unknown-linux-gnu",
    "codex-session-control-aarch64-unknown-linux-gnu",
    "SHA256SUMS",
    "install.sh",
];
const RELEASE_DIR_ENV: &str = "CODEX_SESSION_CONTROL_RELEASE_DIR";

struct BundleFixture {
    _root: tempfile::TempDir,
    directory: PathBuf,
}

impl BundleFixture {
    fn valid() -> Self {
        let root = crate::test_support::private_tempdir();
        let directory = root.path().join("release");
        fs::create_dir(&directory).unwrap();
        fs::write(
            directory.join(RELEASE_ASSETS[0]),
            synthetic_elf(0x003e, b"x86 payload"),
        )
        .unwrap();
        fs::write(
            directory.join(RELEASE_ASSETS[1]),
            synthetic_elf(0x00b7, b"arm payload"),
        )
        .unwrap();
        for binary in &RELEASE_ASSETS[..2] {
            fs::set_permissions(directory.join(binary), fs::Permissions::from_mode(0o700)).unwrap();
        }
        fs::write(directory.join("install.sh"), b"#!/bin/sh\nexit 0\n").unwrap();
        write_checksums(&directory);
        Self {
            _root: root,
            directory,
        }
    }
}

fn synthetic_elf(machine: u16, payload: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0_u8; 64];
    bytes[..7].copy_from_slice(&[0x7f, b'E', b'L', b'F', 2, 1, 1]);
    bytes[16..18].copy_from_slice(&2_u16.to_le_bytes());
    bytes[18..20].copy_from_slice(&machine.to_le_bytes());
    bytes.extend_from_slice(payload);
    bytes
}

fn write_checksums(directory: &Path) {
    let lines = [RELEASE_ASSETS[1], RELEASE_ASSETS[0]]
        .map(|name| {
            let digest = hex::encode(Sha256::digest(fs::read(directory.join(name)).unwrap()));
            format!("{digest}  {name}")
        })
        .join("\n");
    fs::write(directory.join("SHA256SUMS"), format!("{lines}\n")).unwrap();
}

pub(super) fn validate_release_bundle(directory: &Path) -> Result<(), String> {
    let mut actual = fs::read_dir(directory)
        .map_err(|error| format!("cannot read release directory: {error}"))?
        .map(|entry| {
            let entry = entry.map_err(|error| format!("cannot read release entry: {error}"))?;
            if !entry
                .file_type()
                .map_err(|error| format!("cannot inspect release entry: {error}"))?
                .is_file()
            {
                return Err(format!(
                    "release entry is not a regular file: {}",
                    entry.file_name().to_string_lossy()
                ));
            }
            entry
                .file_name()
                .into_string()
                .map_err(|_| "release filename is not UTF-8".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    actual.sort();
    let mut expected = RELEASE_ASSETS.map(str::to_owned).to_vec();
    expected.sort();
    if actual != expected {
        return Err(format!(
            "release assets differ: expected {expected:?}, found {actual:?}"
        ));
    }

    let mut executable_assets = actual
        .iter()
        .filter_map(|name| {
            let mode = fs::metadata(directory.join(name))
                .ok()?
                .permissions()
                .mode();
            (mode & 0o111 != 0).then_some(name.as_str())
        })
        .collect::<Vec<_>>();
    executable_assets.sort();
    let mut expected_executables = RELEASE_ASSETS[..2].to_vec();
    expected_executables.sort();
    if executable_assets != expected_executables {
        return Err(format!(
            "release executables differ: expected {expected_executables:?}, found {executable_assets:?}"
        ));
    }

    let checksums = fs::read_to_string(directory.join("SHA256SUMS"))
        .map_err(|error| format!("cannot read SHA256SUMS: {error}"))?;
    if !checksums.ends_with('\n') {
        return Err("SHA256SUMS must end with one newline".to_owned());
    }
    let lines = checksums.lines().collect::<Vec<_>>();
    let expected_names = [RELEASE_ASSETS[1], RELEASE_ASSETS[0]];
    if lines.len() != expected_names.len() {
        return Err("SHA256SUMS must contain exactly two entries".to_owned());
    }
    for (line, expected_name) in lines.iter().zip(expected_names) {
        let (digest, name) = line
            .split_once("  ")
            .ok_or_else(|| "checksum entry must use two spaces".to_owned())?;
        if name != expected_name
            || digest.len() != 64
            || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
            || digest.bytes().any(|byte| byte.is_ascii_uppercase())
        {
            return Err(format!("invalid checksum entry for {expected_name}"));
        }
        let actual_digest = hex::encode(Sha256::digest(
            fs::read(directory.join(name))
                .map_err(|error| format!("cannot read {name}: {error}"))?,
        ));
        if digest != actual_digest {
            return Err(format!("checksum mismatch for {name}"));
        }
    }

    validate_elf(&directory.join(RELEASE_ASSETS[0]), 0x003e, "x86-64")?;
    validate_elf(&directory.join(RELEASE_ASSETS[1]), 0x00b7, "AArch64")?;
    Ok(())
}

fn validate_elf(path: &Path, expected_machine: u16, label: &str) -> Result<(), String> {
    let bytes = fs::read(path).map_err(|error| format!("cannot read {label} ELF: {error}"))?;
    if bytes.len() < 20
        || bytes[..7] != [0x7f, b'E', b'L', b'F', 2, 1, 1]
        || u16::from_le_bytes([bytes[18], bytes[19]]) != expected_machine
    {
        return Err(format!("invalid {label} ELF header"));
    }
    Ok(())
}

pub(super) fn assert_release_asset_rules() {
    let valid = BundleFixture::valid();
    assert_eq!(validate_release_bundle(&valid.directory), Ok(()));

    let missing = BundleFixture::valid();
    fs::remove_file(missing.directory.join("install.sh")).unwrap();
    assert!(validate_release_bundle(&missing.directory).is_err());

    for extra in ["source.tar.gz", "checksums.txt"] {
        let fixture = BundleFixture::valid();
        fs::write(fixture.directory.join(extra), b"extra").unwrap();
        assert!(validate_release_bundle(&fixture.directory).is_err());
    }

    let executable_extra = BundleFixture::valid();
    let helper = executable_extra.directory.join("release-helper");
    fs::write(&helper, b"helper").unwrap();
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(validate_release_bundle(&executable_extra.directory).is_err());

    let malformed_checksums = BundleFixture::valid();
    let checksums = fs::read_to_string(malformed_checksums.directory.join("SHA256SUMS"))
        .unwrap()
        .to_uppercase();
    fs::write(malformed_checksums.directory.join("SHA256SUMS"), checksums).unwrap();
    assert!(validate_release_bundle(&malformed_checksums.directory).is_err());

    let wrong_order = BundleFixture::valid();
    let checksums = fs::read_to_string(wrong_order.directory.join("SHA256SUMS")).unwrap();
    fs::write(
        wrong_order.directory.join("SHA256SUMS"),
        checksums.lines().rev().collect::<Vec<_>>().join("\n") + "\n",
    )
    .unwrap();
    assert!(validate_release_bundle(&wrong_order.directory).is_err());

    let wrong_elf = BundleFixture::valid();
    fs::write(
        wrong_elf.directory.join(RELEASE_ASSETS[1]),
        synthetic_elf(0x003e, b"wrong architecture"),
    )
    .unwrap();
    write_checksums(&wrong_elf.directory);
    assert!(validate_release_bundle(&wrong_elf.directory).is_err());

    let tampered = BundleFixture::valid();
    fs::write(
        tampered.directory.join(RELEASE_ASSETS[0]),
        synthetic_elf(0x003e, b"tampered"),
    )
    .unwrap();
    assert!(validate_release_bundle(&tampered.directory).is_err());

    let non_executable_binary = BundleFixture::valid();
    fs::set_permissions(
        non_executable_binary.directory.join(RELEASE_ASSETS[0]),
        fs::Permissions::from_mode(0o600),
    )
    .unwrap();
    assert!(validate_release_bundle(&non_executable_binary.directory).is_err());

    let executable_installer = BundleFixture::valid();
    fs::set_permissions(
        executable_installer.directory.join("install.sh"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
    assert!(validate_release_bundle(&executable_installer.directory).is_err());
}

pub(super) fn assert_release_assets() {
    let Some(directory) = std::env::var_os(RELEASE_DIR_ENV) else {
        eprintln!("skipped release bundle: {RELEASE_DIR_ENV} is unset");
        return;
    };
    let directory = PathBuf::from(directory);
    validate_release_bundle(&directory).unwrap();
    eprintln!("validated release bundle: {}", directory.display());
}

#[test]
fn release_asset_rules() {
    assert_release_asset_rules();
    let release = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".github/workflows/release.yml"),
    )
    .unwrap();
    let exact_commands = "expected=\"$(printf '%s\\n' setup update status enable disable uninstall mcp-server codex)\"";
    assert_eq!(
        release.matches(exact_commands).count(),
        2,
        "both native release binaries must expose exactly eight commands"
    );
}
