//! End to end against a small fake service: pairing, the stream, results, Pause,
//! revocation and reconnecting. osquery is replaced by canned rows.

use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use super::*;
use crate::checks::fake::{row, FakeMachine};
use crate::checks::queries::{MAC_MOUNTS, OS_VERSION, SYSTEM_INFO};
use crate::timeutil::format_iso_utc;
use crate::tokens::MemoryStore;

#[derive(Default)]
struct Seen {
    claims: Vec<Value>,
    results: Vec<(String, String, Value)>,
    streams: usize,
}

#[derive(Clone)]
struct FakeService {
    origin: String,
    seen: Arc<Mutex<Seen>>,
    revoked: Arc<AtomicBool>,
    events: Arc<Mutex<Vec<String>>>,
    /// End each stream after sending its events, like a deploy.
    drop_streams: Arc<AtomicBool>,
    sent_events: Arc<AtomicUsize>,
    /// The service's clock minus this computer's.
    skew_ms: i64,
}

fn http_date(ms: i64) -> String {
    const DAYS: [&str; 7] = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let iso = format_iso_utc(ms);
    let month: usize = iso[5..7].parse().unwrap();
    format!(
        "{}, {} {} {} {} GMT",
        DAYS[ms.div_euclid(86_400_000).rem_euclid(7) as usize],
        &iso[8..10],
        MONTHS[month - 1],
        &iso[0..4],
        &iso[11..19]
    )
}

impl FakeService {
    async fn start() -> Self {
        Self::start_with_skew(0).await
    }

    async fn start_with_skew(skew_ms: i64) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let service = FakeService {
            origin: format!("http://{}", listener.local_addr().unwrap()),
            seen: Arc::default(),
            revoked: Arc::default(),
            events: Arc::default(),
            drop_streams: Arc::default(),
            sent_events: Arc::default(),
            skew_ms,
        };
        let accept = service.clone();
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let service = accept.clone();
                tokio::spawn(async move { service.serve(socket).await });
            }
        });
        service
    }

    fn queue(&self, event: Value) {
        self.events
            .lock()
            .unwrap()
            .push(format!("data: {event}\n\n"));
    }

    fn now(&self) -> i64 {
        now_ms() + self.skew_ms
    }

    /// A check request as the service sends it, expiring 30 seconds from the service's now.
    fn check(&self, request_id: &str, approval: Value, checks: Value) -> Value {
        json!({
            "type": "check",
            "requestId": request_id,
            "receipt": format!("receipt-for-{request_id}.sig"),
            "approval": approval,
            "checks": checks,
            "expiresAt": format_iso_utc(self.now() + 30_000),
        })
    }

    async fn serve(self, socket: TcpStream) {
        let mut reader = BufReader::new(socket);
        let mut request_line = String::new();
        if reader.read_line(&mut request_line).await.unwrap_or(0) == 0 {
            return;
        }
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or("").to_string();
        let path = parts.next().unwrap_or("").to_string();
        let mut length = 0usize;
        let mut token = String::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            let (name, value) = line.split_once(':').unwrap();
            match name.to_ascii_lowercase().as_str() {
                "content-length" => length = value.trim().parse().unwrap(),
                "x-wtf-helper-token" => token = value.trim().to_string(),
                _ => {}
            }
        }
        let mut body = vec![0u8; length];
        reader.read_exact(&mut body).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        let mut socket = reader.into_inner();

        let json_reply = |status: &str, payload: Value| {
            let text = payload.to_string();
            format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}", text.len())
        };

        if method == "POST" && path == "/api/wtf/helper/pair/claim" {
            self.seen.lock().unwrap().claims.push(body);
            let reply = json_reply(
                "200 OK",
                json!({ "success": true, "data": { "deviceId": "dev-1", "deviceToken": "tok-1", "userId": "user-000042" } }),
            );
            let _ = socket.write_all(reply.as_bytes()).await;
        } else if method == "GET" && path == "/api/wtf/helper/stream" {
            if self.revoked.load(Ordering::SeqCst) || token != "tok-1" {
                let reply = json_reply(
                    "401 Unauthorized",
                    json!({ "success": false, "error": "This helper was disconnected.", "code": "revoked" }),
                );
                let _ = socket.write_all(reply.as_bytes()).await;
                return;
            }
            self.seen.lock().unwrap().streams += 1;
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nDate: {}\r\nConnection: close\r\n\r\n",
                http_date(self.now())
            );
            if socket.write_all(head.as_bytes()).await.is_err() {
                return;
            }
            let events: Vec<String> = self.events.lock().unwrap().drain(..).collect();
            for event in events {
                self.sent_events.fetch_add(1, Ordering::SeqCst);
                if socket.write_all(event.as_bytes()).await.is_err() {
                    return;
                }
            }
            loop {
                if self.revoked.load(Ordering::SeqCst) || self.drop_streams.load(Ordering::SeqCst) {
                    let _ = socket.shutdown().await;
                    return;
                }
                let beat = format!(
                    "data: {}\n\n",
                    json!({ "type": "heartbeat", "at": format_iso_utc(self.now()) })
                );
                if socket.write_all(beat.as_bytes()).await.is_err() {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        } else if method == "POST" && path.starts_with("/api/wtf/helper/checks/") {
            self.seen.lock().unwrap().results.push((path, token, body));
            let _ = socket
                .write_all(json_reply("200 OK", json!({ "success": true })).as_bytes())
                .await;
        } else {
            let _ = socket
                .write_all(
                    json_reply(
                        "404 Not Found",
                        json!({ "success": false, "code": "not_found" }),
                    )
                    .as_bytes(),
                )
                .await;
        }
    }
}

fn machine() -> FakeMachine {
    FakeMachine::new(Platform::Mac)
        .answer(&SYSTEM_INFO, vec![row(&[("computer_name", "Pat's Mac")])])
        .answer(
            &OS_VERSION,
            vec![row(&[
                ("name", "macOS"),
                ("version", "12.7.3"),
                ("build", "21H1015"),
            ])],
        )
        .answer(
            &MAC_MOUNTS,
            vec![row(&[
                ("path", "/"),
                ("device", "/dev/disk1s1s1"),
                ("type", "apfs"),
                ("blocks_size", "4096"),
                ("blocks", "1000000"),
                ("blocks_available", "500000"),
            ])],
        )
}

fn settings_dir(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("wtf-helper-e2e-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn paired_settings(dir: &std::path::Path, paused: bool) {
    SettingsFile::new(dir)
        .save(&Settings {
            pairing: Some(Pairing {
                host: FLAVOR.host.into(),
                device_id: "dev-1".into(),
                user_id: "user-000042".into(),
                device_name: "Pat's Mac".into(),
                paired_at_ms: 1,
            }),
            paused,
        })
        .unwrap();
}

async fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    let started = Instant::now();
    while !done() {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn pairs_then_answers_a_check_with_the_receipt() {
    let service = FakeService::start().await;
    service.queue(service.check("req-1", json!("person"), json!([{ "check": "disk_space" }])));
    let dir = settings_dir("pair");
    let helper = Helper::new(
        Api::for_tests(&service.origin),
        machine(),
        Box::new(MemoryStore::default()),
        SettingsFile::new(&dir),
        Box::new(|_| {}),
    );

    assert!(helper.pair("12345").await.is_err());
    let status = helper.pair("123 456").await.unwrap();
    assert_eq!(status.paired.as_ref().unwrap().account_hint, "…000042");
    assert_eq!(helper.tokens.get().as_deref(), Some("tok-1"));

    let claim = service.seen.lock().unwrap().claims[0].clone();
    assert_eq!(
        claim,
        json!({ "code": "123456", "deviceName": "Pat's Mac", "os": "macOS", "osVersion": "12.7.3", "helperVersion": HELPER_VERSION })
    );

    wait_for("the result", || {
        !service.seen.lock().unwrap().results.is_empty()
    })
    .await;
    let (path, token, body) = service.seen.lock().unwrap().results[0].clone();
    assert_eq!(path, "/api/wtf/helper/checks/req-1/result");
    assert_eq!(token, "tok-1");
    assert_eq!(body["receipt"], json!("receipt-for-req-1.sig"));
    assert_eq!(body["approval"], json!("person"));
    let results = body["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["check"], json!("disk_space"));
    assert_eq!(results[0]["ok"], json!(true));
    assert!(results[0]["summary"]
        .as_str()
        .unwrap()
        .starts_with("Main drive: 1.9 GB free of 3.8 GB (50%)"));

    wait_for("the activity entry", || {
        !helper.status().activity.is_empty()
    })
    .await;
    let status = helper.status();
    assert_eq!(status.connection, Connection::Connected);
    let entry = &status.activity[0];
    assert_eq!(entry.checks, vec!["Disk space"]);
    assert_eq!(entry.approval.as_deref(), Some("You allowed"));
    assert_eq!(entry.outcome, "Sent");
    // The log keeps what ran and who allowed it, never the output.
    assert!(!serde_json::to_string(&status).unwrap().contains("GB free"));
    let saved = std::fs::read_to_string(dir.join("settings.json")).unwrap();
    assert!(!saved.contains("tok-1") && !saved.contains("GB"));

    helper.disconnect(None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn refuses_everything_while_paused_even_allow_all() {
    let service = FakeService::start().await;
    service.queue(service.check(
        "req-2",
        json!("allow_all"),
        json!([{ "check": "disk_space" }, { "check": "installed_apps" }]),
    ));
    let dir = settings_dir("paused");
    paired_settings(&dir, true);
    let tokens = MemoryStore::default();
    tokens.set("tok-1").unwrap();
    let helper = Helper::new(
        Api::for_tests(&service.origin),
        machine(),
        Box::new(tokens),
        SettingsFile::new(&dir),
        Box::new(|_| {}),
    );
    helper.start_stream();

    wait_for("the refusal", || {
        !service.seen.lock().unwrap().results.is_empty()
    })
    .await;
    let (_, _, body) = service.seen.lock().unwrap().results[0].clone();
    assert_eq!(body["approval"], json!("allow_all"));
    assert_eq!(
        body["results"],
        json!([
            { "check": "disk_space", "ok": false, "error": "WTF Helper is paused on this computer." },
            { "check": "installed_apps", "ok": false, "error": "WTF Helper is paused on this computer." },
        ])
    );
    wait_for("the activity entry", || {
        !helper.status().activity.is_empty()
    })
    .await;
    let entry = &helper.status().activity[0];
    assert_eq!(entry.outcome, "Refused: paused");
    assert_eq!(entry.approval.as_deref(), Some("Allow all"));
    assert!(
        helper.machine.asked.lock().unwrap().is_empty(),
        "nothing may run while paused"
    );

    helper.disconnect(None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn posts_nothing_for_a_check_without_approval() {
    let service = FakeService::start().await;
    let mut event = service.check("req-3", json!("person"), json!([{ "check": "disk_space" }]));
    event.as_object_mut().unwrap().remove("approval");
    service.queue(event);
    let dir = settings_dir("noapproval");
    paired_settings(&dir, false);
    let tokens = MemoryStore::default();
    tokens.set("tok-1").unwrap();
    let helper = Helper::new(
        Api::for_tests(&service.origin),
        machine(),
        Box::new(tokens),
        SettingsFile::new(&dir),
        Box::new(|_| {}),
    );
    helper.start_stream();

    wait_for("the activity entry", || {
        !helper.status().activity.is_empty()
    })
    .await;
    assert_eq!(helper.status().activity[0].outcome, "Refused: no approval");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(service.seen.lock().unwrap().results.is_empty());
    assert!(helper.machine.asked.lock().unwrap().is_empty());

    helper.disconnect(None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn forgets_the_pairing_when_the_service_revokes_it() {
    let service = FakeService::start().await;
    let dir = settings_dir("revoked");
    paired_settings(&dir, false);
    let tokens = MemoryStore::default();
    tokens.set("tok-1").unwrap();
    let helper = Helper::new(
        Api::for_tests(&service.origin),
        machine(),
        Box::new(tokens),
        SettingsFile::new(&dir),
        Box::new(|_| {}),
    );
    helper.start_stream();
    wait_for("the stream", || {
        helper.status().connection == Connection::Connected
    })
    .await;

    service.revoked.store(true, Ordering::SeqCst);
    wait_for("the pairing to end", || {
        helper.status().connection == Connection::NotPaired
    })
    .await;
    let status = helper.status();
    assert!(status.paired.is_none());
    assert!(status
        .notice
        .unwrap()
        .contains("disconnected from your collaborAItr account"));
    assert!(helper.tokens.get().is_none());
    assert!(SettingsFile::new(&dir).load().pairing.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn reconnects_after_the_stream_drops() {
    let service = FakeService::start().await;
    service.drop_streams.store(true, Ordering::SeqCst);
    let dir = settings_dir("reconnect");
    paired_settings(&dir, false);
    let tokens = MemoryStore::default();
    tokens.set("tok-1").unwrap();
    let helper = Helper::new(
        Api::for_tests(&service.origin),
        machine(),
        Box::new(tokens),
        SettingsFile::new(&dir),
        Box::new(|_| {}),
    );
    helper.start_stream();

    wait_for("a second connection", || {
        service.seen.lock().unwrap().streams >= 2
    })
    .await;
    service.drop_streams.store(false, Ordering::SeqCst);
    service.queue(service.check("req-4", json!("person"), json!([{ "check": "disk_space" }])));
    wait_for("the check after reconnecting", || {
        !service.seen.lock().unwrap().results.is_empty()
    })
    .await;
    assert!(helper.is_paired());

    helper.disconnect(None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn answers_checks_when_this_computers_clock_is_five_minutes_fast() {
    // Service clock 5 minutes behind this computer: by local time every request looks expired.
    let service = FakeService::start_with_skew(-300_000).await;
    service.queue(service.check("req-5", json!("person"), json!([{ "check": "disk_space" }])));
    let dir = settings_dir("skew");
    paired_settings(&dir, false);
    let tokens = MemoryStore::default();
    tokens.set("tok-1").unwrap();
    let helper = Helper::new(
        Api::for_tests(&service.origin),
        machine(),
        Box::new(tokens),
        SettingsFile::new(&dir),
        Box::new(|_| {}),
    );
    helper.start_stream();

    wait_for("the result", || {
        !service.seen.lock().unwrap().results.is_empty()
    })
    .await;
    let (_, _, body) = service.seen.lock().unwrap().results[0].clone();
    assert_eq!(body["results"][0]["ok"], json!(true));
    let offset = helper.clock_offset_ms.load(Ordering::SeqCst);
    assert!((offset + 300_000).abs() < 2_000, "offset {offset}");

    helper.disconnect(None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn disconnect_stops_the_stream_and_clears_the_token() {
    let service = FakeService::start().await;
    let dir = settings_dir("disconnect");
    paired_settings(&dir, false);
    let tokens = MemoryStore::default();
    tokens.set("tok-1").unwrap();
    let helper = Helper::new(
        Api::for_tests(&service.origin),
        machine(),
        Box::new(tokens),
        SettingsFile::new(&dir),
        Box::new(|_| {}),
    );
    helper.start_stream();
    wait_for("the stream", || {
        helper.status().connection == Connection::Connected
    })
    .await;

    let status = helper.disconnect(None);
    assert_eq!(status.connection, Connection::NotPaired);
    assert!(helper.tokens.get().is_none());
    let streams = service.seen.lock().unwrap().streams;
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    assert_eq!(
        service.seen.lock().unwrap().streams,
        streams,
        "no reconnect after Disconnect"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
