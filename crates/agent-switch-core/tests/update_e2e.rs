//! End-to-end test of the update pipeline against a LOCAL in-process
//! HTTP server: download → SHA-256 verify → zip extract → binary replace.
//! The server is a minimal TcpListener thread (no external process: some
//! sandboxes hang on `python3 -m http.server`'s reverse-DNS banner lookup,
//! and CI images may lack Python entirely), so the test always runs.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use agent_switch_core::update::{
    asset_for, download, extract_cli_binary, replace_binary, verify_against_sums, Asset,
    PackageKind, Release,
};

const PAYLOAD: &[u8] = b"#!/bin/sh\necho fake agent-switch 99.0.0\n";

fn build_zip(dir: &std::path::Path, binary_name: &str, content: &[u8]) -> std::path::PathBuf {
    let zip_path = dir.join("test-payload.zip");
    let file = std::fs::File::create(&zip_path).unwrap();
    {
        let mut zip = zip::ZipWriter::new(file);
        // Stored keeps the test independent of a specific encoder.
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file(binary_name.to_string(), opts).unwrap();
        zip.write_all(content).unwrap();
        zip.finish().unwrap();
    }
    zip_path
}

/// Minimal single-threaded static file server: answers one request per
/// connection with `Connection: close`, serving files from `dir` by name.
fn serve_dir(dir: &std::path::Path, listener: TcpListener, stop: Arc<AtomicBool>) {
    use std::io::ErrorKind;
    let _ = listener.set_nonblocking(true);
    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        match listener.accept() {
            Ok((stream, _)) => handle_request(stream, dir),
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => return,
        }
    }
}

fn handle_request(mut stream: TcpStream, dir: &std::path::Path) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut got = Vec::new();
    let mut buf = [0u8; 1024];
    // Read until the end of the HTTP header (we only handle GET, no body).
    while !got.windows(4).any(|w| w == b"\r\n\r\n") {
        match stream.read(&mut buf) {
            Ok(0) => return,
            Ok(n) => {
                got.extend_from_slice(&buf[..n]);
                if got.len() > 8192 {
                    return;
                }
            }
            Err(_) => return,
        }
    }
    let request = String::from_utf8_lossy(&got);
    let path = request.split_whitespace().nth(1).unwrap_or("/");
    let file = dir.join(path.trim_start_matches('/'));
    match std::fs::read(&file) {
        Ok(body) => {
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(&body);
        }
        Err(_) => {
            let _ = stream
                .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        }
    }
    let _ = stream.flush();
}

/// Bind an ephemeral port, serve `dir` in a background thread, and wait
/// until the port accepts connections. Returns (base_url, stop flag, join
/// handle); dropping the stop flag's Arc and setting it ends the thread.
fn start_server(dir: &std::path::Path) -> (String, Arc<AtomicBool>, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
    let port = listener.local_addr().unwrap().port();
    let stop = Arc::new(AtomicBool::new(false));
    let dir = dir.to_path_buf();
    let stop_clone = stop.clone();
    let handle = std::thread::spawn(move || serve_dir(&dir, listener, stop_clone));
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if TcpStream::connect_timeout(
            &format!("127.0.0.1:{port}").parse().unwrap(),
            Duration::from_millis(200),
        )
        .is_ok()
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    (
        format!("http://127.0.0.1:{port}"),
        stop,
        handle,
    )
}

#[test]
fn update_pipeline_download_verify_extract_replace() {
    let dir = std::env::temp_dir().join(format!(
        "as-update-e2e-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    std::fs::create_dir_all(&dir).unwrap();

    // --- serve: the release zip + its SHA256SUMS.txt ---
    let zip_path = build_zip(&dir, "agent-switch", PAYLOAD);
    let digest = agent_switch_core::update::sha256_file(&zip_path).unwrap();
    std::fs::write(
        dir.join("SHA256SUMS.txt"),
        format!("{digest}  test-payload.zip\n"),
    )
    .unwrap();
    let (base, stop, server) = start_server(&dir);

    let release = Release {
        tag_name: "v99.0.0".into(),
        html_url: "http://example.invalid/".into(),
        assets: vec![
            Asset {
                name: "test-payload.zip".into(),
                browser_download_url: format!("{base}/test-payload.zip"),
            },
            Asset {
                name: "SHA256SUMS.txt".into(),
                browser_download_url: format!("{base}/SHA256SUMS.txt"),
            },
        ],
    };

    // --- download with progress ---
    let got = dir.join("dl/test-payload.zip");
    std::fs::create_dir_all(got.parent().unwrap()).unwrap();
    let mut progress_calls = 0u64;
    let done = download(
        &release.assets[0].browser_download_url,
        &got,
        |_, _| progress_calls += 1,
    )
    .unwrap();
    assert_eq!(done, std::fs::metadata(&got).unwrap().len());
    assert!(done > 0 && progress_calls > 0);

    // --- checksum: verify, then a corrupted file must fail ---
    assert_eq!(
        verify_against_sums(&release, "test-payload.zip", &got).unwrap(),
        Some(digest.clone())
    );
    let corrupt = dir.join("dl/bad.zip");
    let mut bytes = std::fs::read(&got).unwrap();
    *bytes.last_mut().unwrap() ^= 0xff;
    std::fs::write(&corrupt, &bytes).unwrap();
    assert!(verify_against_sums(&release, "test-payload.zip", &corrupt).is_err());

    // --- asset selection matches the (fake) release for this platform ---
    let want_name = format!(
        "agent-switch-99.0.0-{}-{}.zip",
        std::env::consts::ARCH,
        if std::env::consts::OS == "macos" {
            "apple-darwin"
        } else {
            "unknown-linux-gnu"
        }
    );
    if std::env::consts::OS == "macos" || std::env::consts::OS == "linux" {
        let named = Release {
            assets: vec![Asset {
                name: want_name.clone(),
                browser_download_url: "http://example.invalid/x".into(),
            }],
            ..release.clone()
        };
        assert_eq!(
            asset_for(&named, PackageKind::Cli)
                .unwrap()
                .name,
            want_name
        );
    }

    // --- extract the binary out of the zip ---
    let new_bin = dir.join("out/agent-switch");
    std::fs::create_dir_all(new_bin.parent().unwrap()).unwrap();
    extract_cli_binary(&got, &new_bin).unwrap();
    assert_eq!(std::fs::read(&new_bin).unwrap(), PAYLOAD);

    // --- replace an existing "installed" binary in place ---
    let install = dir.join("install");
    std::fs::create_dir_all(&install).unwrap();
    let current = install.join("agent-switch");
    std::fs::write(&current, b"old binary").unwrap();
    replace_binary(&current, &new_bin).unwrap();
    assert_eq!(std::fs::read(&current).unwrap(), PAYLOAD);

    // The accept loop re-checks the stop flag every 10 ms.
    stop.store(true, Ordering::Relaxed);
    let _ = server.join();
    let _ = std::fs::remove_dir_all(&dir);
}
