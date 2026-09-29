use serde_json::{json, Value};
use std::time::Duration;
use sweep_server::{serve, ServeOptions, ServerHandle};

const TOKEN: &str = "test-token-0123456789";

struct Fixture {
    handle: ServerHandle,
    client: reqwest::Client,
    _dist: tempfile::TempDir,
}

async fn start(exit_on_idle: bool, idle_ms: u64) -> Fixture {
    let dist = tempfile::tempdir().unwrap();
    std::fs::write(
        dist.path().join("index.html"),
        "<html>clearsweep-test</html>",
    )
    .unwrap();
    std::fs::write(dist.path().join("app.js"), "console.log(1)").unwrap();
    let (url, handle) = serve(ServeOptions {
        port: 0,
        token: TOKEN.into(),
        exit_on_idle,
        dist_override: Some(dist.path().to_path_buf()),
        idle_timeout: Duration::from_millis(idle_ms),
        ctx: None,
    })
    .await
    .unwrap();
    assert!(url.starts_with("http://127.0.0.1:"));
    assert!(url.ends_with(&format!("/?t={TOKEN}")));
    Fixture {
        handle,
        client: reqwest::Client::new(),
        _dist: dist,
    }
}

impl Fixture {
    fn url(&self, path: &str) -> String {
        format!("{}{}", self.handle.base_url(), path)
    }
    fn call_req(&self, method: &str, params: Value) -> reqwest::RequestBuilder {
        self.client
            .post(self.url("/api/call"))
            .header("X-Sweep-Token", TOKEN)
            .json(&json!({"callId": "c1", "method": method, "params": params}))
    }
}

fn parse_lines(body: &str) -> Vec<Value> {
    body.lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[tokio::test]
async fn missing_token_is_401() {
    let f = start(false, 30_000).await;
    let r = f
        .client
        .post(f.url("/api/call"))
        .json(&json!({"callId":"1","method":"api.methods"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    f.handle.shutdown();
}

#[tokio::test]
async fn wrong_token_is_401_on_every_api_route() {
    let f = start(false, 30_000).await;
    for path in ["/api/call", "/api/cancel", "/api/heartbeat"] {
        let r = f
            .client
            .post(f.url(path))
            .header("X-Sweep-Token", "wrong")
            .json(&json!({"callId":"1","method":"api.methods"}))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 401, "{path}");
    }
    // same length, one char off
    let almost = format!("{}X", &TOKEN[..TOKEN.len() - 1]);
    let r = f
        .client
        .post(f.url("/api/heartbeat"))
        .header("X-Sweep-Token", almost)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    f.handle.shutdown();
}

#[tokio::test]
async fn bad_host_is_403_everywhere() {
    let f = start(false, 30_000).await;
    let r = f
        .call_req("api.methods", Value::Null)
        .header("Host", "evil.example.com")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    // right host, wrong port
    let r = f
        .call_req("api.methods", Value::Null)
        .header("Host", "127.0.0.1:1")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    // static route is protected too
    let r = f
        .client
        .get(f.url("/"))
        .header("Host", "evil.example.com")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    // localhost:<port> is accepted
    let r = f
        .call_req("api.methods", Value::Null)
        .header("Host", format!("localhost:{}", f.handle.port))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    f.handle.shutdown();
}

#[tokio::test]
async fn bad_origin_is_403_good_origin_ok() {
    let f = start(false, 30_000).await;
    let r = f
        .call_req("api.methods", Value::Null)
        .header("Origin", "http://evil.example.com")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    let r = f
        .call_req("api.methods", Value::Null)
        .header("Origin", "null")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);
    for origin in [
        format!("http://127.0.0.1:{}", f.handle.port),
        format!("http://localhost:{}", f.handle.port),
    ] {
        let r = f
            .call_req("api.methods", Value::Null)
            .header("Origin", origin)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
    }
    f.handle.shutdown();
}

#[tokio::test]
async fn good_call_streams_ndjson_result() {
    let f = start(false, 30_000).await;
    let r = f.call_req("sysinfo.get", Value::Null).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(
        r.headers()["content-type"].to_str().unwrap(),
        "application/x-ndjson"
    );
    let lines = parse_lines(&r.text().await.unwrap());
    let last = lines.last().unwrap();
    assert_eq!(last["type"], "result");
    let v = &last["value"];
    assert!(v["cpu"]["cores"].as_u64().unwrap() >= 1);
    assert!(v["memory"]["total"].as_u64().unwrap() > 0);
    assert!(v["uptimeSecs"].is_u64());
    f.handle.shutdown();
}

#[tokio::test]
async fn progress_lines_precede_single_result() {
    let f = start(false, 30_000).await;
    let r = f
        .call_req("api.sleep", json!({"ms": 60}))
        .send()
        .await
        .unwrap();
    let lines = parse_lines(&r.text().await.unwrap());
    assert!(lines.len() >= 3, "{lines:?}");
    let (last, progress) = lines.split_last().unwrap();
    assert_eq!(last["type"], "result");
    assert_eq!(last["value"]["slept"], 60);
    for p in progress {
        assert_eq!(p["type"], "progress");
        assert_eq!(p["stage"], "sleep");
    }
    f.handle.shutdown();
}

#[tokio::test]
async fn unknown_method_and_stub_yield_error_lines() {
    let f = start(false, 30_000).await;
    let r = f
        .call_req("nope.nothing", Value::Null)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let lines = parse_lines(&r.text().await.unwrap());
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["type"], "error");
    assert_eq!(lines[0]["error"]["code"], "NotFound");

    let r = f.call_req("startup.list", json!({})).send().await.unwrap();
    let lines = parse_lines(&r.text().await.unwrap());
    assert_eq!(lines[0]["error"]["code"], "NotImplemented");
    f.handle.shutdown();
}

#[tokio::test]
async fn cancel_endpoint_stops_running_call() {
    let f = start(false, 30_000).await;
    let started = std::time::Instant::now();
    let call = f.call_req("api.sleep", json!({"ms": 20000})).send();
    let task = tokio::spawn(async move { call.await.unwrap().text().await.unwrap() });
    tokio::time::sleep(Duration::from_millis(300)).await;
    let r = f
        .client
        .post(f.url("/api/cancel"))
        .header("X-Sweep-Token", TOKEN)
        .json(&json!({"callId": "c1"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: Value = r.json().await.unwrap();
    assert_eq!(body["found"], true);
    let text = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("call should end promptly after cancel")
        .unwrap();
    let lines = parse_lines(&text);
    let last = lines.last().unwrap();
    assert_eq!(last["type"], "error");
    assert_eq!(last["error"]["code"], "Cancelled");
    assert!(started.elapsed() < Duration::from_secs(10));

    // cancelling an unknown id is not an error
    let r = f
        .client
        .post(f.url("/api/cancel"))
        .header("X-Sweep-Token", TOKEN)
        .json(&json!({"callId": "does-not-exist"}))
        .send()
        .await
        .unwrap();
    let body: Value = r.json().await.unwrap();
    assert_eq!(body["found"], false);
    f.handle.shutdown();
}

#[tokio::test]
async fn static_files_and_spa_fallback() {
    let f = start(false, 30_000).await;
    let r = f.client.get(f.url("/")).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.headers()["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/html"));
    assert!(r.text().await.unwrap().contains("clearsweep-test"));
    let r = f.client.get(f.url("/some/route")).send().await.unwrap();
    assert!(r.text().await.unwrap().contains("clearsweep-test"));
    let r = f.client.get(f.url("/app.js")).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let r = f.client.get(f.url("/missing.js")).send().await.unwrap();
    assert_eq!(r.status(), 404);
    let r = f
        .client
        .get(f.url("/..%2f..%2fCargo.toml"))
        .send()
        .await
        .unwrap();
    assert_ne!(r.status(), 200);
    let r = f.client.get(f.url("/api/unknown")).send().await.unwrap();
    assert_eq!(r.status(), 401); // /api is never served static content
    f.handle.shutdown();
}

#[tokio::test]
async fn exits_when_heartbeats_stop() {
    let f = start(true, 400).await;
    let r = f
        .client
        .post(f.url("/api/heartbeat"))
        .header("X-Sweep-Token", TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);
    tokio::time::timeout(Duration::from_secs(10), f.handle.wait())
        .await
        .expect("server should exit after heartbeats stop");
}

#[tokio::test]
async fn does_not_exit_without_any_heartbeat() {
    let f = start(true, 200).await;
    tokio::time::sleep(Duration::from_millis(800)).await;
    let r = f.call_req("api.methods", Value::Null).send().await.unwrap();
    assert_eq!(r.status(), 200);
    f.handle.shutdown();
}
