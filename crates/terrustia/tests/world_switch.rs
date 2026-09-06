//! World switching, end to end, against the real compiled binary — not a unit test of the
//! `ServerEvent::PanelSwitchWorld` handler (that's `game::server::panel_admin_events` in
//! `src/game/server.rs`) and not a mock of the restart. This is the one claim that only a real
//! subprocess can prove: that hitting the panel's switch endpoint on a *running* `terrustia`
//! process actually replaces it — same PID, on Unix, via `exec` — with a fresh process serving a
//! different world file, having already saved the one it was running.
//!
//! Runs with `HOME`/`XDG_DATA_HOME`/`USERPROFILE` redirected at a scratch directory, the same
//! isolation `new_world_cli.rs` already uses, so this never touches the machine's real Terraria
//! worlds.

#![cfg(unix)]

mod support;

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

fn scratch_home() -> PathBuf {
    support::scratch_dir("terrustia-world-switch")
}

fn find_named(dir: &Path, name: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(find_named(&path, name));
        } else if path.file_name().and_then(|n| n.to_str()) == Some(name) {
            found.push(path);
        }
    }
    found
}

fn wait_for_file(dir: &Path, name: &str, timeout: Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if find_named(dir, name)
            .iter()
            .any(|p| std::fs::metadata(p).is_ok_and(|m| m.len() > 0))
        {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Generate one small world under `name`, the same way `new_world_cli.rs` does, and wait for the
/// file to actually land before moving on — the switch test needs both worlds to genuinely exist
/// on disk beforehand, not merely be requested.
fn generate_world(home: &Path, name: &str, listen: &str) {
    std::fs::write(
        home.join("terrustia.toml"),
        "autosave_secs = 1\nworld_width = 400\nworld_height = 300\n",
    )
    .expect("write config");
    let mut child = Command::new(env!("CARGO_BIN_EXE_terrustia"))
        .args(["--new", name, "--listen", listen])
        .current_dir(home)
        .env("HOME", home)
        .env("XDG_DATA_HOME", home.join("xdg"))
        .env("USERPROFILE", home)
        .env_remove("TERRUSTIA_LOG")
        // No test may depend on the network. Both of these are `tokio::spawn`ed at boot, so left
        // on, every server spawned here makes a real GitHub request and multicasts for a UPnP
        // gateway. This is hygiene, not a fix for anything: the CLI-test flake recorded in TODO.md
        // was measured with both already off and still failed 5 runs in 8, so the cause is elsewhere.
        .env("TERRUSTIA_UPDATE_CHECK_ENABLED", "false")
        .env("TERRUSTIA_UPNP_ENABLED", "false")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn terrustia --new");
    let filename = format!("{}.wld", name.replace(' ', "_"));
    let landed = wait_for_file(home, &filename, Duration::from_secs(30));
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        landed,
        "expected {filename} to exist under {}",
        home.display()
    );
}

/// Every line a subprocess writes to stdout, delivered as it is written rather than only once the
/// process exits — this server never exits on its own, so waiting for it to finish before reading
/// its output (the way `new_world_cli.rs` does for the short-lived `--new` runs) would simply
/// never return.
fn stream_stdout(child: &mut Child) -> mpsc::Receiver<String> {
    let stdout = child.stdout.take().expect("captured stdout");
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

/// The one-time claim token this server prints on its own console at startup — parsed out of the
/// real stdout stream, the same way an operator would actually read it, rather than reached
/// through a test-only backdoor.
fn wait_for_claim_token(lines: &mpsc::Receiver<String>, timeout: Duration) -> String {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if let Ok(line) = lines.recv_timeout(Duration::from_millis(200))
            && let Some(rest) = line.split_once("/register <name> <password> ")
            && let Some(token) = rest.1.split_whitespace().next()
        {
            return token.to_string();
        }
    }
    panic!("the server never printed a claim token within {timeout:?}");
}

async fn wait_for_port(addr: &str, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    while tokio::time::Instant::now() < deadline {
        if tokio::net::TcpStream::connect(addr).await.is_ok() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

/// The same minimal HTTP client `tests/panel.rs` uses — this workspace has no HTTP client
/// dependency, and this is three requests.
mod http {
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    pub async fn post_json(url: &str, json: &str, session: Option<&str>) -> (u16, String) {
        request("POST", url, Some(json), session).await
    }

    pub async fn get(url: &str, session: Option<&str>) -> (u16, String) {
        request("GET", url, None, session).await
    }

    async fn request(
        method: &str,
        url: &str,
        json_body: Option<&str>,
        session: Option<&str>,
    ) -> (u16, String) {
        let (host_port, path) = split_url(url);
        let mut stream =
            tokio::time::timeout(Duration::from_secs(5), TcpStream::connect(&host_port))
                .await
                .expect("connect timed out")
                .expect("connect failed");

        let mut request =
            format!("{method} {path} HTTP/1.1\r\nHost: {host_port}\r\nConnection: close\r\n");
        if let Some(session) = session {
            request.push_str(&format!("Authorization: Bearer {session}\r\n"));
        }
        match json_body {
            Some(json) => {
                request.push_str("Content-Type: application/json\r\n");
                request.push_str(&format!("Content-Length: {}\r\n\r\n{json}", json.len()));
            }
            None => request.push_str("\r\n"),
        }

        stream
            .write_all(request.as_bytes())
            .await
            .expect("write failed");
        let mut raw = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut raw))
            .await
            .expect("read timed out")
            .expect("read failed");
        let raw = String::from_utf8_lossy(&raw);

        let mut parts = raw.splitn(2, "\r\n\r\n");
        let head = parts.next().unwrap_or_default();
        let body = parts.next().unwrap_or_default().to_string();
        let status = head
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|code| code.parse().ok())
            .unwrap_or(0);
        (status, body)
    }

    fn split_url(url: &str) -> (String, String) {
        let rest = url.strip_prefix("http://").unwrap_or(url);
        match rest.find('/') {
            Some(i) => (rest[..i].to_string(), rest[i..].to_string()),
            None => (rest.to_string(), "/".to_string()),
        }
    }
}

fn extract_field<'a>(body: &'a str, key: &str) -> &'a str {
    let needle = format!("\"{key}\":\"");
    let start = body
        .find(&needle)
        .unwrap_or_else(|| panic!("no {key:?} in {body}"))
        + needle.len();
    let end = body[start..].find('"').unwrap() + start;
    &body[start..end]
}

#[tokio::test(flavor = "multi_thread")]
async fn switching_worlds_from_the_panel_restarts_the_real_process_into_the_new_world() {
    let home = scratch_home();
    std::fs::create_dir_all(&home).expect("scratch home");

    // Two real worlds, generated up front — the switch has to find both on disk.
    generate_world(&home, "World Alpha", &support::free_addr());
    generate_world(&home, "World Beta", &support::free_addr());

    let game_addr = &support::free_addr();
    let panel_addr = &support::free_addr();
    std::fs::write(
        home.join("terrustia.toml"),
        format!("autosave_secs = 0\npanel_enabled = true\npanel_listen = \"{panel_addr}\"\n"),
    )
    .expect("write config");

    let child = Command::new(env!("CARGO_BIN_EXE_terrustia"))
        .args(["--world", "World Alpha", "--listen", game_addr])
        .current_dir(&home)
        .env("HOME", &home)
        .env("XDG_DATA_HOME", home.join("xdg"))
        .env("USERPROFILE", &home)
        .env_remove("TERRUSTIA_LOG")
        .env("TERRUSTIA_UPDATE_CHECK_ENABLED", "false")
        .env("TERRUSTIA_UPNP_ENABLED", "false")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn terrustia");
    // Killed on drop, so a failing assertion below cannot leave it holding its port.
    let mut child = support::Reaper(child);
    let pid_before = child.id();
    let stdout = stream_stdout(&mut child);

    assert!(
        wait_for_port(panel_addr, Duration::from_secs(20)).await,
        "the panel should come up"
    );
    let token = wait_for_claim_token(&stdout, Duration::from_secs(10));

    let base = format!("http://{panel_addr}");
    let (status, body) = http::post_json(
        &format!("{base}/api/login"),
        &format!(
            r#"{{"name":"admin","password":"correcthorsebatterystaple","claim_token":"{token}"}}"#
        ),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "claiming the fresh server should succeed: {body}"
    );
    let session = extract_field(&body, "session").to_string();

    // Confirmed serving World Alpha before touching anything.
    let (status, body) = http::get(&format!("{base}/api/status"), Some(&session)).await;
    assert_eq!(status, 200, "{body}");
    assert!(
        body.contains("World Alpha") || body.contains("World_Alpha"),
        "expected World Alpha to be running first: {body}"
    );

    // A name that is not on disk is refused, before touching the real switch.
    let (status, body) = http::post_json(
        &format!("{base}/api/worlds/switch"),
        r#"{"name":"No Such World"}"#,
        Some(&session),
    )
    .await;
    assert_eq!(status, 404, "an unknown world name must be refused: {body}");

    // The real switch: World Alpha -> World Beta.
    let (status, body) = http::post_json(
        &format!("{base}/api/worlds/switch"),
        r#"{"name":"World_Beta"}"#,
        Some(&session),
    )
    .await;
    assert_eq!(status, 200, "the switch itself should be accepted: {body}");

    // The process saves, stops, and `exec`s a replacement — the panel and game ports both drop
    // briefly and then come back on the same PID, now serving World Beta.
    assert!(
        wait_for_port(panel_addr, Duration::from_secs(20)).await,
        "the panel should come back up after the restart"
    );
    assert_eq!(
        child.id(),
        pid_before,
        "exec() replaces the process image but keeps the PID — this must still be the same OS \
         process, not a new one"
    );

    // A brand-new session, because the switch signed everyone out — this is a genuinely new
    // process, and the in-memory session map from before it did not survive.
    let token_after = wait_for_claim_token(&stdout, Duration::from_secs(10));
    let (status, body) = http::post_json(
        &format!("{base}/api/login"),
        &format!(
            r#"{{"name":"admin","password":"correcthorsebatterystaple","claim_token":"{token_after}"}}"#
        ),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "the account file did not travel with the process: {body}"
    );
    let session_after = extract_field(&body, "session").to_string();

    let (status, body) = http::get(&format!("{base}/api/status"), Some(&session_after)).await;
    assert_eq!(status, 200, "{body}");
    assert!(
        body.contains("World Beta") || body.contains("World_Beta"),
        "expected the restarted process to be serving World Beta: {body}"
    );

    drop(child);
    let _ = std::fs::remove_dir_all(&home);
}
