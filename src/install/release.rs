use std::{
    fs,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    time::Duration,
};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::{
    ControllerError,
    paths::{FileKind, validate_existing},
};

pub(super) const RELEASE_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
pub(super) const RELEASE_METADATA_TIMEOUT: Duration = Duration::from_secs(30);
pub(super) const RELEASE_TRANSFER_IDLE_TIMEOUT: Duration = Duration::from_secs(30);
pub(super) const RELEASE_REPOSITORY: &str = "agentlehub/codex-session-control";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ReleaseStage {
    #[cfg(test)]
    Connect,
    Metadata,
    Download,
    TransferIdle,
}

impl ReleaseStage {
    const fn name(self) -> &'static str {
        match self {
            #[cfg(test)]
            Self::Connect => "release-connect",
            Self::Metadata => "release-metadata",
            Self::Download => "release-download",
            Self::TransferIdle => "release-download transfer idle",
        }
    }

    const fn timeout(self) -> Duration {
        match self {
            #[cfg(test)]
            Self::Connect => RELEASE_CONNECT_TIMEOUT,
            Self::Metadata => RELEASE_METADATA_TIMEOUT,
            Self::Download | Self::TransferIdle => RELEASE_TRANSFER_IDLE_TIMEOUT,
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct ReleaseEndpoints {
    pub(super) api: String,
    pub(super) downloads: String,
}

pub(super) fn production_release_endpoints() -> ReleaseEndpoints {
    ReleaseEndpoints {
        api: "https://api.github.com".to_owned(),
        downloads: format!("https://github.com/{RELEASE_REPOSITORY}"),
    }
}

#[derive(Clone, Debug)]
pub(super) struct ReleaseAsset {
    pub(super) name: String,
    pub(super) url: String,
    pub(super) size: u64,
}

#[derive(Clone, Debug)]
pub(super) struct VerifiedRelease {
    #[cfg(test)]
    pub(super) tag: String,
    pub(super) version: semver::Version,
    pub(super) target: String,
    pub(super) binary: ReleaseAsset,
    pub(super) checksums: ReleaseAsset,
}

#[derive(Clone, Debug)]
pub(super) struct DownloadedRelease {
    pub(super) binary_path: PathBuf,
    #[cfg(test)]
    pub(super) checksums_path: PathBuf,
    #[cfg(test)]
    pub(super) sha256: String,
}

#[derive(Debug, thiserror::Error)]
pub(super) enum ReleaseDownloadError {
    #[error(transparent)]
    Download(ControllerError),
    #[error(transparent)]
    Integrity(ControllerError),
}

impl ReleaseDownloadError {
    pub(super) fn into_controller_error(self) -> ControllerError {
        match self {
            Self::Download(error) | Self::Integrity(error) => error,
        }
    }
}

#[derive(Deserialize)]
struct GithubReleaseMetadata {
    tag_name: String,
    assets: Vec<GithubReleaseAsset>,
}

#[derive(Deserialize)]
struct GithubReleaseAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

pub(super) fn release_target_for_arch(architecture: &str) -> Result<&'static str, ControllerError> {
    match architecture {
        "x86_64" => Ok("x86_64-unknown-linux-gnu"),
        "aarch64" => Ok("aarch64-unknown-linux-gnu"),
        _ => Err(ControllerError::InvalidData {
            field: "architecture",
            reason: "unsupported release target",
        }),
    }
}

pub(super) fn build_release_client() -> Result<reqwest::Client, ControllerError> {
    reqwest::Client::builder()
        .connect_timeout(RELEASE_CONNECT_TIMEOUT)
        .user_agent(concat!("codex-session-control/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|_| {
            ControllerError::Operational("release client initialization failed".to_owned())
        })
}

pub(super) async fn with_release_stage_timeout<T, F>(
    stage: ReleaseStage,
    future: F,
) -> Result<T, ControllerError>
where
    F: std::future::Future<Output = Result<T, ControllerError>>,
{
    tokio::time::timeout(stage.timeout(), future)
        .await
        .map_err(|_| ControllerError::Operational(format!("{} timed out", stage.name())))?
}

pub(super) async fn discover_latest_release(
    client: &reqwest::Client,
    endpoints: &ReleaseEndpoints,
    target: &str,
) -> Result<VerifiedRelease, ControllerError> {
    let url = format!(
        "{}/repos/{RELEASE_REPOSITORY}/releases/latest",
        endpoints.api.trim_end_matches('/'),
    );
    let metadata: GithubReleaseMetadata =
        with_release_stage_timeout(ReleaseStage::Metadata, async {
            let response = client.get(url).send().await.map_err(|error| {
                if error.is_connect() || error.is_timeout() {
                    ControllerError::Operational("release-connect timed out".to_owned())
                } else {
                    ControllerError::Operational("release-metadata request failed".to_owned())
                }
            })?;
            response
                .error_for_status()
                .map_err(|_| {
                    ControllerError::Operational("release-metadata request failed".to_owned())
                })?
                .json()
                .await
                .map_err(|_| {
                    ControllerError::Operational("release-metadata response is invalid".to_owned())
                })
        })
        .await?;

    let version_text = metadata.tag_name.strip_prefix('v').ok_or_else(|| {
        ControllerError::Operational("release-metadata tag is invalid".to_owned())
    })?;
    let version = semver::Version::parse(version_text)
        .map_err(|_| ControllerError::Operational("release-metadata tag is invalid".to_owned()))?;
    if format!("v{version}") != metadata.tag_name {
        return Err(ControllerError::Operational(
            "release-metadata tag is not canonical".to_owned(),
        ));
    }
    let binary_name = format!("codex-session-control-{target}");
    let downloads = endpoints.downloads.trim_end_matches('/');
    let expected_binary_url = format!(
        "{downloads}/releases/download/{}/{binary_name}",
        metadata.tag_name
    );
    let expected_checksums_url = format!(
        "{downloads}/releases/download/{}/SHA256SUMS",
        metadata.tag_name
    );
    let binary = exact_release_asset(&metadata.assets, &binary_name, &expected_binary_url)?;
    let checksums = exact_release_asset(&metadata.assets, "SHA256SUMS", &expected_checksums_url)?;
    Ok(VerifiedRelease {
        #[cfg(test)]
        tag: metadata.tag_name.clone(),
        version,
        target: target.to_owned(),
        binary,
        checksums,
    })
}

fn exact_release_asset(
    assets: &[GithubReleaseAsset],
    name: &str,
    expected_url: &str,
) -> Result<ReleaseAsset, ControllerError> {
    let mut matching = assets.iter().filter(|asset| asset.name == name);
    let asset = matching.next().ok_or_else(|| {
        ControllerError::Operational(format!("release-metadata asset is missing: {name}"))
    })?;
    if matching.next().is_some() || asset.browser_download_url != expected_url || asset.size == 0 {
        return Err(ControllerError::Operational(format!(
            "release-metadata asset is invalid: {name}"
        )));
    }
    Ok(ReleaseAsset {
        name: asset.name.clone(),
        url: asset.browser_download_url.clone(),
        size: asset.size,
    })
}

pub(super) async fn download_verified_release(
    client: &reqwest::Client,
    release: &VerifiedRelease,
    directory: &Path,
) -> Result<DownloadedRelease, ReleaseDownloadError> {
    let binary_path = directory.join(&release.binary.name);
    let checksums_path = directory.join(&release.checksums.name);
    let sha256 = stream_release_asset(
        client,
        &release.binary,
        &binary_path,
        ReleaseStage::Download,
    )
    .await
    .map_err(ReleaseDownloadError::Download)?;
    if let Err(error) = stream_release_asset(
        client,
        &release.checksums,
        &checksums_path,
        ReleaseStage::Download,
    )
    .await
    {
        let _ = fs::remove_file(&binary_path);
        return Err(ReleaseDownloadError::Download(error));
    }
    if let Err(error) = verify_release_integrity(&checksums_path, &release.binary.name, &sha256) {
        let _ = fs::remove_file(&binary_path);
        let _ = fs::remove_file(&checksums_path);
        return Err(ReleaseDownloadError::Integrity(error));
    }
    Ok(DownloadedRelease {
        binary_path,
        #[cfg(test)]
        checksums_path,
        #[cfg(test)]
        sha256,
    })
}

pub(super) fn verify_release_integrity(
    checksums_path: &Path,
    binary_name: &str,
    sha256: &str,
) -> Result<(), ControllerError> {
    fs::read(checksums_path)
        .map_err(|_| ControllerError::Operational("checksum file cannot be read".to_owned()))
        .and_then(|checksums| validate_checksum_entry(&checksums, binary_name, sha256))
}

pub(super) async fn stream_release_asset(
    client: &reqwest::Client,
    asset: &ReleaseAsset,
    path: &Path,
    stage: ReleaseStage,
) -> Result<String, ControllerError> {
    let mut response = with_release_stage_timeout(ReleaseStage::TransferIdle, async {
        client.get(&asset.url).send().await.map_err(|error| {
            if error.is_connect() || error.is_timeout() {
                ControllerError::Operational("release-connect timed out".to_owned())
            } else {
                ControllerError::Operational(format!("{} request failed", stage.name()))
            }
        })
    })
    .await?
    .error_for_status()
    .map_err(|_| ControllerError::Operational(format!("{} request failed", stage.name())))?;
    if response
        .content_length()
        .is_some_and(|content_length| content_length != asset.size)
    {
        return Err(ControllerError::Operational(format!(
            "{} content length does not match immutable metadata",
            stage.name()
        )));
    }

    let parent = path.parent().ok_or(ControllerError::InvalidData {
        field: "release_path",
        reason: "has no parent",
    })?;
    validate_existing(
        parent,
        FileKind::Directory,
        rustix::process::geteuid().as_raw(),
    )?;
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|_| {
            ControllerError::Operational(format!("{} destination cannot be created", stage.name()))
        })?;
    let mut file = tokio::fs::File::from_std(file);
    let mut digest = Sha256::new();
    let mut received = 0_u64;
    let result = async {
        loop {
            let chunk = with_release_stage_timeout(ReleaseStage::TransferIdle, async {
                response.chunk().await.map_err(|_| {
                    ControllerError::Operational(format!("{} transfer failed", stage.name()))
                })
            })
            .await?;
            let Some(chunk) = chunk else {
                break;
            };
            received = received.checked_add(chunk.len() as u64).ok_or_else(|| {
                ControllerError::Operational(format!("{} byte count overflow", stage.name()))
            })?;
            if received > asset.size {
                return Err(ControllerError::Operational(format!(
                    "{} exceeds immutable metadata size",
                    stage.name()
                )));
            }
            use tokio::io::AsyncWriteExt;
            file.write_all(&chunk).await.map_err(|_| {
                ControllerError::Operational(format!("{} destination write failed", stage.name()))
            })?;
            digest.update(&chunk);
        }
        if received != asset.size {
            return Err(ControllerError::Operational(format!(
                "{} is shorter than immutable metadata size",
                stage.name()
            )));
        }
        use tokio::io::AsyncWriteExt;
        file.flush().await.map_err(|_| {
            ControllerError::Operational(format!("{} destination flush failed", stage.name()))
        })?;
        file.sync_all().await.map_err(|_| {
            ControllerError::Operational(format!("{} destination sync failed", stage.name()))
        })?;
        Ok(hex::encode(digest.finalize()))
    }
    .await;
    if result.is_err() {
        drop(file);
        let _ = fs::remove_file(path);
    }
    result
}

pub(super) fn validate_checksum_entry(
    bytes: &[u8],
    asset_name: &str,
    expected_digest: &str,
) -> Result<(), ControllerError> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| ControllerError::Operational("checksum file is not UTF-8".to_owned()))?;
    if !text.ends_with('\n') {
        return Err(ControllerError::Operational(
            "checksum entry is malformed".to_owned(),
        ));
    }
    let mut matched = 0_u8;
    for line in text.lines() {
        let Some((digest, name)) = line.split_once("  ") else {
            return Err(ControllerError::Operational(
                "checksum entry is malformed".to_owned(),
            ));
        };
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            || name.is_empty()
            || name.contains(char::is_whitespace)
        {
            return Err(ControllerError::Operational(
                "checksum entry is malformed".to_owned(),
            ));
        }
        if name == asset_name {
            matched = matched.saturating_add(1);
            if digest != expected_digest {
                return Err(ControllerError::Operational(
                    "checksum does not match release asset".to_owned(),
                ));
            }
        }
    }
    if matched != 1 {
        return Err(ControllerError::Operational(
            "checksum file must contain exactly one matching entry".to_owned(),
        ));
    }
    Ok(())
}
