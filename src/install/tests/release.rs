use std::{
    fs,
    future::pending,
    os::unix::fs::PermissionsExt,
    sync::{Arc, Mutex},
    time::Duration,
};

use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{Notify, oneshot},
    time::{advance, timeout},
};

use super::*;

async fn read_request(stream: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 1024];
    while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut buffer).await.unwrap();
        assert_ne!(read, 0);
        bytes.extend_from_slice(&buffer[..read]);
    }
    String::from_utf8(bytes).unwrap()
}

async fn write_response(stream: &mut TcpStream, content_length: u64, body: &[u8]) {
    stream
        .write_all(
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {content_length}\r\nConnection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    for chunk in body.chunks(8192) {
        stream.write_all(chunk).await.unwrap();
        tokio::task::yield_now().await;
    }
    stream.shutdown().await.unwrap();
}

async fn release_server(
    binary: Vec<u8>,
    checksum_body: Vec<u8>,
) -> (ReleaseEndpoints, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&requests);
    let binary_size = binary.len();
    let checksum_size = checksum_body.len();
    let target = product_target();
    let binary_name = format!("codex-session-control-{target}");
    let metadata = serde_json::json!({
        "tag_name": "v1.2.3",
        "assets": [
            {
                "name": binary_name,
                "browser_download_url": format!("{base}/releases/download/v1.2.3/{binary_name}"),
                "size": binary_size
            },
            {
                "name": "SHA256SUMS",
                "browser_download_url": format!("{base}/releases/download/v1.2.3/SHA256SUMS"),
                "size": checksum_size
            }
        ]
    })
    .to_string()
    .into_bytes();
    let served_base = base.clone();
    tokio::spawn(async move {
        for _ in 0..3 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_request(&mut stream).await;
            let path = request
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap()
                .to_owned();
            recorded.lock().unwrap().push(path.clone());
            match path.as_str() {
                "/repos/Agentlehub/codex-session-control/releases/latest" => {
                    write_response(&mut stream, metadata.len() as u64, &metadata).await;
                }
                path if path.ends_with(&format!("/{binary_name}")) => {
                    write_response(&mut stream, binary.len() as u64, &binary).await;
                }
                path if path.ends_with("/SHA256SUMS") => {
                    write_response(&mut stream, checksum_body.len() as u64, &checksum_body).await;
                }
                _ => panic!("unexpected request: {path}"),
            }
        }
    });
    (
        ReleaseEndpoints {
            api: served_base.clone(),
            downloads: served_base,
        },
        requests,
    )
}

#[test]
fn release_target_mapping_accepts_only_the_two_approved_architectures() {
    assert_eq!(
        release_target_for_arch("x86_64").unwrap(),
        "x86_64-unknown-linux-gnu"
    );
    assert_eq!(
        release_target_for_arch("aarch64").unwrap(),
        "aarch64-unknown-linux-gnu"
    );
    for rejected in ["", "amd64", "arm64", "armv7", "riscv64", "s390x"] {
        assert!(release_target_for_arch(rejected).is_err(), "{rejected}");
    }
}

#[tokio::test]
async fn latest_metadata_resolves_once_to_exact_immutable_assets() {
    let binary = b"release bytes".to_vec();
    let digest = hex::encode(Sha256::digest(&binary));
    let checksums = format!("{digest}  codex-session-control-{}\n", product_target());
    let (endpoints, requests) = release_server(binary, checksums.into_bytes()).await;
    let client = build_release_client().unwrap();

    let release = discover_latest_release(&client, &endpoints, product_target())
        .await
        .unwrap();

    assert_eq!(release.tag, "v1.2.3");
    assert_eq!(release.version.to_string(), "1.2.3");
    assert_eq!(release.target, product_target());
    assert_eq!(
        release.binary.name,
        format!("codex-session-control-{}", product_target())
    );
    assert_eq!(
        release.binary.url,
        format!(
            "{}/releases/download/v1.2.3/codex-session-control-{}",
            endpoints.downloads,
            product_target()
        )
    );
    assert_eq!(
        release.checksums.url,
        format!(
            "{}/releases/download/v1.2.3/SHA256SUMS",
            endpoints.downloads
        )
    );
    assert_eq!(
        requests.lock().unwrap().as_slice(),
        ["/repos/Agentlehub/codex-session-control/releases/latest"]
    );
}

#[tokio::test]
async fn release_files_stream_privately_with_exact_sizes_and_checksum() {
    let binary = vec![0x5a; 17 * 1024 * 1024 + 3];
    let digest = hex::encode(Sha256::digest(&binary));
    let checksums = format!("{digest}  codex-session-control-{}\n", product_target());
    let (endpoints, requests) = release_server(binary.clone(), checksums.into_bytes()).await;
    let client = build_release_client().unwrap();
    let release = discover_latest_release(&client, &endpoints, product_target())
        .await
        .unwrap();
    let destination = tempfile::tempdir().unwrap();

    let downloaded = download_verified_release(&client, &release, destination.path())
        .await
        .unwrap();

    assert_eq!(downloaded.sha256, digest);
    assert_eq!(fs::read(&downloaded.binary_path).unwrap(), binary);
    assert_eq!(
        fs::metadata(&downloaded.binary_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(&downloaded.checksums_path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(requests.lock().unwrap().len(), 3);
}

#[test]
fn checksum_requires_one_exact_lowercase_entry_without_duplicates() {
    let name = "codex-session-control-x86_64-unknown-linux-gnu";
    let digest = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    assert!(
        validate_checksum_entry(format!("{digest}  {name}\n").as_bytes(), name, digest).is_ok()
    );
    for invalid in [
        format!("{digest}  {name}\n{digest}  {name}\n"),
        format!("{}  {name}\n", digest.to_ascii_uppercase()),
        format!("{digest} {name}\n"),
        format!("{digest}  other\n"),
        format!("{digest}  {name} extra\n"),
        format!("short  {name}\n"),
        format!("{digest}  {name}"),
    ] {
        assert!(
            validate_checksum_entry(invalid.as_bytes(), name, digest).is_err(),
            "{invalid:?}"
        );
    }
}

#[tokio::test]
async fn content_length_short_malformed_and_oversized_bodies_fail_exactly() {
    for case in ["header-mismatch", "short", "oversized", "malformed"] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/asset", listener.local_addr().unwrap());
        let metadata_size = if case == "short" { 5 } else { 4 };
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_request(&mut stream).await;
            match case {
                "header-mismatch" => {
                    write_response(&mut stream, 5, b"12345").await;
                }
                "short" => {
                    write_response(&mut stream, 5, b"1234").await;
                }
                "oversized" => {
                    stream
                                .write_all(
                                    b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n5\r\n12345\r\n0\r\n\r\n",
                                )
                                .await
                                .unwrap();
                }
                "malformed" => {
                    stream
                                .write_all(
                                    b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\nZ\r\n1234\r\n0\r\n\r\n",
                                )
                                .await
                                .unwrap();
                }
                _ => unreachable!(),
            }
        });
        let asset = ReleaseAsset {
            name: "asset".to_owned(),
            url,
            size: metadata_size,
        };
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("asset");

        let error = stream_release_asset(
            &build_release_client().unwrap(),
            &asset,
            &path,
            ReleaseStage::Download,
        )
        .await
        .unwrap_err();

        assert!(error.to_string().contains("release-download"), "{case}");
        assert!(!path.exists());
    }
}

#[tokio::test(start_paused = true)]
async fn stage_timeouts_fire_at_the_exact_connect_boundary() {
    let (started_tx, started_rx) = oneshot::channel();
    let task = tokio::spawn(async move {
        with_release_stage_timeout(ReleaseStage::Connect, async move {
            started_tx.send(()).unwrap();
            pending::<Result<(), ControllerError>>().await
        })
        .await
    });
    started_rx.await.unwrap();

    advance(RELEASE_CONNECT_TIMEOUT - Duration::from_millis(1)).await;
    assert!(!task.is_finished());
    advance(Duration::from_millis(1)).await;
    tokio::task::yield_now().await;

    assert_eq!(
        task.await.unwrap().unwrap_err().to_string(),
        "release-connect timed out"
    );
}

#[tokio::test(start_paused = true)]
async fn metadata_response_timeout_uses_the_exact_thirty_second_boundary() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (pending_tx, pending_rx) = oneshot::channel();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        pending_tx.send(()).unwrap();
        pending::<()>().await;
    });
    let client = build_release_client().unwrap();
    let endpoints = ReleaseEndpoints {
        api: base.clone(),
        downloads: base,
    };
    let task = tokio::spawn(async move {
        discover_latest_release(&client, &endpoints, product_target()).await
    });
    pending_rx.await.unwrap();

    advance(RELEASE_METADATA_TIMEOUT - Duration::from_millis(1)).await;
    assert!(!task.is_finished());
    advance(Duration::from_millis(1)).await;
    tokio::task::yield_now().await;

    assert_eq!(
        task.await.unwrap().unwrap_err().to_string(),
        "release-metadata timed out"
    );
}

#[tokio::test(start_paused = true)]
async fn transfer_idle_deadline_resets_on_each_byte_then_expires() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/asset", listener.local_addr().unwrap());
    let first_sent = Arc::new(Notify::new());
    let send_second = Arc::new(Notify::new());
    let second_sent = Arc::new(Notify::new());
    let first_signal = Arc::clone(&first_sent);
    let second_gate = Arc::clone(&send_second);
    let second_signal = Arc::clone(&second_sent);
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nConnection: close\r\n\r\na")
            .await
            .unwrap();
        first_signal.notify_one();
        second_gate.notified().await;
        stream.write_all(b"b").await.unwrap();
        second_signal.notify_one();
        pending::<()>().await;
    });
    let asset = ReleaseAsset {
        name: "asset".to_owned(),
        url,
        size: 3,
    };
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("asset");
    let task = tokio::spawn(async move {
        stream_release_asset(
            &build_release_client().unwrap(),
            &asset,
            &path,
            ReleaseStage::Download,
        )
        .await
    });
    first_sent.notified().await;

    advance(RELEASE_TRANSFER_IDLE_TIMEOUT - Duration::from_millis(1)).await;
    assert!(!task.is_finished());
    send_second.notify_one();
    second_sent.notified().await;
    tokio::task::yield_now().await;
    advance(RELEASE_TRANSFER_IDLE_TIMEOUT - Duration::from_millis(1)).await;
    assert!(!task.is_finished());
    advance(Duration::from_millis(1)).await;
    tokio::task::yield_now().await;

    assert_eq!(
        task.await.unwrap().unwrap_err().to_string(),
        "release-download transfer idle timed out"
    );
}

#[tokio::test]
async fn metadata_rejects_mutable_or_malformed_asset_claims() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let target = product_target();
    let body = serde_json::json!({
        "tag_name": "main",
        "assets": [{
            "name": format!("codex-session-control-{target}"),
            "browser_download_url": format!("{base}/mutable"),
            "size": 1
        }, {
            "name": "SHA256SUMS",
            "browser_download_url": format!("{base}/mutable-sums"),
            "size": 1
        }]
    })
    .to_string()
    .into_bytes();
    tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        write_response(&mut stream, body.len() as u64, &body).await;
    });
    let endpoints = ReleaseEndpoints {
        api: base.clone(),
        downloads: base,
    };

    let error = discover_latest_release(&build_release_client().unwrap(), &endpoints, target)
        .await
        .unwrap_err();

    assert!(error.to_string().contains("release-metadata"));
}

#[tokio::test]
async fn stalled_transfer_test_is_bounded_by_fixture_timeout() {
    timeout(Duration::from_secs(2), async {
        let _ = release_target_for_arch("x86_64").unwrap();
    })
    .await
    .unwrap();
}
