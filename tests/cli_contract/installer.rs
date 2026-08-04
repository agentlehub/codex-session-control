use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

use sha2::{Digest, Sha256};

const TAG: &str = "v1.2.3";
const REPOSITORY: &str = "Agentlehub/codex-session-control";

struct Fixture {
    _root: tempfile::TempDir,
    root: PathBuf,
    bin: PathBuf,
    tmp: PathBuf,
    candidate_log: PathBuf,
    curl_log: PathBuf,
    sha_log: PathBuf,
    verified: PathBuf,
    fail_checksums: PathBuf,
    fail_setup: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let root_path = root.path().to_path_buf();
        let bin = root_path.join("bin");
        let tmp = root_path.join("tmp");
        fs::create_dir(&bin).unwrap();
        fs::create_dir(&tmp).unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o700)).unwrap();
        let fixture = Self {
            _root: root,
            root: root_path.clone(),
            bin,
            tmp,
            candidate_log: root_path.join("candidate.log"),
            curl_log: root_path.join("curl.log"),
            sha_log: root_path.join("sha.log"),
            verified: root_path.join("verified"),
            fail_checksums: root_path.join("fail-checksums"),
            fail_setup: root_path.join("fail-setup"),
        };
        fixture.write_fakes();
        fixture
    }

    fn installer() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("install.sh")
    }

    fn write_fakes(&self) {
        fs::write(self.root.join("uname-system"), b"Linux\n").unwrap();
        fs::write(self.root.join("uname-machine"), b"x86_64\n").unwrap();
        write_executable(
            &self.bin.join("uname"),
            &format!(
                "#!/bin/sh\ncase \"$1\" in -s) cat {} ;; -m) cat {} ;; *) exit 64 ;; esac\n",
                shell_quote(&self.root.join("uname-system")),
                shell_quote(&self.root.join("uname-machine")),
            ),
        );

        let candidate = self.root.join("candidate");
        write_executable(
            &candidate,
            &format!(
                "#!/bin/sh\n\
test \"$#\" -eq 1\n\
test \"$1\" = setup\n\
test -f {verified}\n\
candidate_mode=$(stat -c %a \"$0\")\n\
candidate_dir=$(dirname \"$0\")\n\
directory_mode=$(stat -c %a \"$candidate_dir\")\n\
printf '%s\\n%s\\n%s\\n%s\\n' \"$0\" \"$*\" \"$candidate_mode\" \"$directory_mode\" > {candidate_log}\n\
test ! -f {fail_setup}\n",
                verified = shell_quote(&self.verified),
                candidate_log = shell_quote(&self.candidate_log),
                fail_setup = shell_quote(&self.fail_setup),
            ),
        );
        let digest = hex::encode(Sha256::digest(fs::read(&candidate).unwrap()));
        fs::write(
            self.root.join("SHA256SUMS"),
            format!(
                "{digest}  codex-session-control-x86_64-unknown-linux-gnu\n\
{digest}  codex-session-control-aarch64-unknown-linux-gnu\n"
            ),
        )
        .unwrap();

        write_executable(
            &self.bin.join("curl"),
            &format!(
                "#!/bin/sh\n\
printf '%s\\n' \"$*\" >> {curl_log}\n\
output=\n\
url=\n\
while test \"$#\" -gt 0; do\n\
  case \"$1\" in\n\
    --output) output=$2; shift 2 ;;\n\
    --fail|--location) shift ;;\n\
    --connect-timeout|--speed-time|--speed-limit) shift 2 ;;\n\
    *) url=$1; shift ;;\n\
  esac\n\
done\n\
case \"$url\" in\n\
  https://api.github.com/repos/{repository}/releases/latest)\n\
    printf '%s\\n' '{{\"tag_name\":\"{tag}\"}}'\n\
    ;;\n\
  https://github.com/{repository}/releases/download/{tag}/SHA256SUMS)\n\
    if test -f {fail_checksums}; then exit 22; fi\n\
    cp {checksums} \"$output\"\n\
    ;;\n\
  https://github.com/{repository}/releases/download/{tag}/codex-session-control-*-unknown-linux-gnu)\n\
    cp {candidate} \"$output\"\n\
    ;;\n\
  *) exit 65 ;;\n\
esac\n",
                curl_log = shell_quote(&self.curl_log),
                repository = REPOSITORY,
                tag = TAG,
                fail_checksums = shell_quote(&self.fail_checksums),
                checksums = shell_quote(&self.root.join("SHA256SUMS")),
                candidate = shell_quote(&candidate),
            ),
        );
        write_executable(
            &self.bin.join("sha256sum"),
            &format!(
                "#!/bin/sh\n\
printf '%s\\n' \"$*\" >> {sha_log}\n\
/usr/bin/sha256sum \"$@\"\n\
status=$?\n\
if test \"$status\" -eq 0; then : > {verified}; fi\n\
exit \"$status\"\n",
                sha_log = shell_quote(&self.sha_log),
                verified = shell_quote(&self.verified),
            ),
        );
    }

    fn set_platform(&self, system: &str, machine: &str) {
        fs::write(self.root.join("uname-system"), format!("{system}\n")).unwrap();
        fs::write(self.root.join("uname-machine"), format!("{machine}\n")).unwrap();
    }

    fn run(&self) -> Output {
        let path = format!("{}:/usr/bin:/bin", self.bin.display());
        Command::new("sh")
            .arg(Self::installer())
            .env_clear()
            .env("PATH", path)
            .env("HOME", &self.root)
            .env("TMPDIR", &self.tmp)
            .output()
            .unwrap()
    }

    fn curl_log(&self) -> String {
        fs::read_to_string(&self.curl_log).unwrap_or_default()
    }
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\"'\"'"))
}

fn write_executable(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn installer_requires_linux_and_maps_only_approved_architectures() {
    let fixture = Fixture::new();
    fixture.set_platform("Darwin", "x86_64");
    let output = fixture.run();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "codex-session-control supports Linux only\n"
    );
    assert!(fixture.curl_log().is_empty());

    for rejected in ["amd64", "arm64", "riscv64", "s390x"] {
        let fixture = Fixture::new();
        fixture.set_platform("Linux", rejected);
        let output = fixture.run();
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(
            String::from_utf8(output.stderr).unwrap(),
            format!("unsupported architecture: {rejected}\n")
        );
        assert!(fixture.curl_log().is_empty());
    }

    for (machine, target) in [
        ("x86_64", "x86_64-unknown-linux-gnu"),
        ("aarch64", "aarch64-unknown-linux-gnu"),
    ] {
        let fixture = Fixture::new();
        fixture.set_platform("Linux", machine);
        let output = fixture.run();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            fixture
                .curl_log()
                .contains(&format!("codex-session-control-{target}"))
        );
    }
}

#[test]
fn installer_verifies_one_immutable_release_before_setup_and_cleans_success() {
    let fixture = Fixture::new();

    let output = fixture.run();

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let curl = fixture.curl_log();
    assert_eq!(
        curl.matches(&format!(
            "https://api.github.com/repos/{REPOSITORY}/releases/latest"
        ))
        .count(),
        1
    );
    assert_eq!(curl.lines().count(), 3);
    for line in curl.lines() {
        assert!(line.contains("--fail --location"));
        assert!(line.contains("--connect-timeout 10"));
        assert!(line.contains("--speed-time 30"));
        assert!(line.contains("--speed-limit 1"));
        assert!(!line.contains("releases/latest/download"));
    }
    for line in curl.lines().skip(1) {
        assert!(line.contains(&format!("/releases/download/{TAG}/")));
    }
    assert_eq!(
        fs::read_to_string(&fixture.sha_log).unwrap(),
        "--check SHA256SUMS.selected\n"
    );
    assert!(fixture.verified.is_file());

    let candidate = fs::read_to_string(&fixture.candidate_log).unwrap();
    let fields = candidate.lines().collect::<Vec<_>>();
    assert_eq!(fields[1], "setup");
    assert_eq!(fields[2], "700");
    assert_eq!(fields[3], "700");
    let candidate_path = PathBuf::from(fields[0]);
    assert!(candidate_path.is_absolute());
    assert!(!candidate_path.parent().unwrap().exists());
    assert!(String::from_utf8(output.stderr).unwrap().is_empty());
}

#[test]
fn installer_preserves_exact_retry_boundaries() {
    let before_candidate = Fixture::new();
    fs::write(&before_candidate.fail_checksums, b"fail").unwrap();
    let output = before_candidate.run();
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(
        stderr,
        format!(
            "Bootstrap failed before candidate verification.\nRetry: {}\n",
            Fixture::installer().display()
        )
    );
    assert_eq!(fs::read_dir(&before_candidate.tmp).unwrap().count(), 0);
    assert!(!before_candidate.candidate_log.exists());

    let after_verification = Fixture::new();
    fs::write(&after_verification.fail_setup, b"fail").unwrap();
    let output = after_verification.run();
    assert_eq!(output.status.code(), Some(1));
    let candidate = fs::read_to_string(&after_verification.candidate_log).unwrap();
    let candidate_path = PathBuf::from(candidate.lines().next().unwrap());
    let directory = candidate_path.parent().unwrap();
    assert!(candidate_path.is_file());
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        format!(
            "Verified candidate preserved after setup failure.\nRetry: {} setup\nCleanup: rm -rf {}\n",
            candidate_path.display(),
            directory.display()
        )
    );
    assert!(directory.is_dir());
}

#[test]
fn installer_rejects_stdin_and_contains_no_extra_bootstrap_behavior() {
    let script = fs::read(Fixture::installer()).unwrap();
    let root = tempfile::tempdir().unwrap();
    let output = Command::new("sh")
        .arg("-s")
        .current_dir(root.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write as _;
            child.stdin.as_mut().unwrap().write_all(&script)?;
            child.wait_with_output()
        })
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "installer must be run from a regular file\n"
    );

    let script = String::from_utf8(script).unwrap();
    for forbidden in [
        ".profile",
        ".bashrc",
        ".zshrc",
        "npm ",
        "pnpm ",
        "cargo install",
        "/main/",
        "latest/download",
    ] {
        assert!(!script.contains(forbidden), "{forbidden}");
    }
    let verification = script.find("sha256sum --check").unwrap();
    let execution = script.find("\"$candidate\" setup").unwrap();
    assert!(verification < execution);
    assert_eq!(script.matches("\"$candidate\" setup").count(), 1);
}
