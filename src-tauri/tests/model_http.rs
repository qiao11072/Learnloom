use learnloom_m0::{
    config::Snapshot,
    model,
    store::{Grade, Question, Wiki},
    workflow,
};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::{oneshot, Mutex},
};
use tokio_util::sync::CancellationToken;

async fn request(socket: &mut tokio::net::TcpStream) -> (String, Value) {
    let mut data = vec![];
    let mut part = [0u8; 1024];
    let header_end;
    loop {
        let n = socket.read(&mut part).await.unwrap();
        assert!(n > 0);
        data.extend_from_slice(&part[..n]);
        if let Some(p) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            header_end = p + 4;
            break;
        }
    }
    let header = String::from_utf8(data[..header_end].to_vec()).unwrap();
    let length: usize = header
        .lines()
        .find_map(|l| {
            l.to_lowercase()
                .strip_prefix("content-length:")
                .map(|v| v.trim().parse().unwrap())
        })
        .unwrap();
    while data.len() < header_end + length {
        let n = socket.read(&mut part).await.unwrap();
        data.extend_from_slice(&part[..n]);
    }
    (
        header,
        serde_json::from_slice(&data[header_end..header_end + length]).unwrap(),
    )
}
async fn reply(socket: &mut tokio::net::TcpStream, status: u16, body: &str, content_type: &str) {
    let wire=format!("HTTP/1.1 {status} Status\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
    let _ = socket.write_all(wire.as_bytes()).await;
}
async fn server(status: u16, content: Value) -> (Snapshot, tokio::task::JoinHandle<Value>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let (_, value) = request(&mut socket).await;
        reply(
            &mut socket,
            status,
            &content.to_string(),
            "application/json",
        )
        .await;
        value
    });
    (
        Snapshot {
            endpoint,
            ..Default::default()
        },
        task,
    )
}
fn envelope(content: &str) -> Value {
    json!({"choices":[{"finish_reason":"stop","message":{"content":content}}],"usage":{"prompt_tokens":20,"completion_tokens":10,"total_tokens":30},"model":"m0-mock"})
}
#[tokio::test]
async fn request_shape_structured_output_and_version_are_observed() {
    let (s, task) = server(200, envelope("{\"ok\":true}")).await;
    let result = model::generate(
        &s,
        "Return JSON",
        &json!({"task":"probe"}),
        Duration::from_secs(2),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let sent = task.await.unwrap();
    assert_eq!(sent["model"], s.model);
    assert_eq!(sent["stream"], false);
    assert_eq!(sent["max_tokens"], 1024);
    assert_eq!(sent["response_format"]["type"], "json_object");
    assert_eq!(result.config_revision, s.config_revision);
    assert_eq!(result.content, "{\"ok\":true}");
}
#[tokio::test]
async fn provider_errors_invalid_json_and_truncation_are_not_successes() {
    for (status, expected) in [
        (401, "AUTH_FAILED"),
        (403, "AUTH_FAILED"),
        (404, "MODEL_UNAVAILABLE"),
        (429, "RATE_LIMITED"),
        (500, "PROVIDER_ERROR"),
    ] {
        let (s, t) = server(status, json!({"error":"fixture"})).await;
        assert_eq!(
            model::generate(
                &s,
                "JSON",
                &json!({}),
                Duration::from_secs(2),
                CancellationToken::new()
            )
            .await
            .unwrap_err(),
            expected
        );
        t.await.unwrap();
    }
    for response in [
        envelope("not json"),
        json!({"choices":[{"finish_reason":"length","message":{"content":"{}"}}]}),
    ] {
        let (s, t) = server(200, response).await;
        assert!(model::generate(
            &s,
            "JSON",
            &json!({}),
            Duration::from_secs(2),
            CancellationToken::new()
        )
        .await
        .unwrap_err()
        .contains("INVALID_OUTPUT"));
        t.await.unwrap();
    }
}
#[tokio::test]
async fn timeouts_and_cancellation_use_server_barriers() {
    for cancel_case in [true, false] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let s = Snapshot {
            endpoint: format!("http://{}", listener.local_addr().unwrap()),
            ..Default::default()
        };
        let (received_tx, received_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel::<()>();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            request(&mut socket).await;
            let _ = received_tx.send(());
            let _ = release_rx.await;
            reply(
                &mut socket,
                200,
                &envelope("{}").to_string(),
                "application/json",
            )
            .await;
        });
        let token = CancellationToken::new();
        let child = token.clone();
        let client = tokio::spawn(async move {
            model::generate(&s, "JSON", &json!({}), Duration::from_millis(300), child).await
        });
        received_rx.await.unwrap();
        if cancel_case {
            token.cancel();
        }
        let error = client.await.unwrap().unwrap_err();
        assert_eq!(error, if cancel_case { "CANCELLED" } else { "TIMEOUT" });
        let _ = release_tx.send(());
        server.await.unwrap();
    }
}
#[tokio::test]
async fn streaming_chinese_utf8_and_missing_done() {
    for done in [true, false] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let s = Snapshot {
            endpoint: format!("http://{}", listener.local_addr().unwrap()),
            ..Default::default()
        };
        let t = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            request(&mut socket).await;
            let mut body = format!(
                "data: {}\n\n",
                json!({"choices":[{"delta":{"content":"中文流式"}}]})
            );
            if done {
                body.push_str("data: [DONE]\n\n");
            }
            let head=format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",body.len());
            socket.write_all(head.as_bytes()).await.unwrap();
            for byte in body.as_bytes() {
                socket.write_all(&[*byte]).await.unwrap();
                tokio::task::yield_now().await;
            }
        });
        let result = model::stream(&s, Duration::from_secs(2), CancellationToken::new()).await;
        if done {
            assert_eq!(result.unwrap().concat(), "中文流式");
        } else {
            assert_eq!(result.unwrap_err(), "STREAM_INTERRUPTED");
        }
        t.await.unwrap();
    }
}
#[tokio::test]
async fn mock_source_to_mastery_chain_is_persisted() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let s = Snapshot {
        endpoint: format!("http://{}", listener.local_addr().unwrap()),
        ..Default::default()
    };
    let w = Wiki {
        page_id: "chain-page".into(),
        point_id: "chain-point".into(),
        title: "催化剂".into(),
        scope: "chemistry".into(),
        body: "催化剂降低反应路径的活化能，使平衡更快到达，但不改变平衡常数和平衡位置。".into(),
        evidence: "不改变平衡常数".into(),
    };
    let q = Question {
        point_id: "chain-point".into(),
        prompt: "催化剂怎样影响活化能和平衡？".into(),
        reference: "降低活化能，平衡不变。".into(),
        explanation: "改变路径。".into(),
        required: vec!["降低活化能".into(), "平衡不变".into()],
        critical_errors: vec!["改变平衡".into()],
    };
    let g = Grade {
        score: 0,
        matched: vec![],
        missing: vec![0, 1],
        critical: vec![],
        noncritical_errors: vec![],
        explanation: "没有回答知识要点。".into(),
        standard: "grading_standard_v1".into(),
    };
    let responses = vec![
        serde_json::to_string(&w).unwrap(),
        serde_json::to_string(&q).unwrap(),
        json!({"matched":[],"missing":["R0","R1"],"critical":[],"noncritical_errors":[],"explanation":g.explanation}).to_string(),
    ];
    let seen = Arc::new(Mutex::new(vec![]));
    let capture = seen.clone();
    let task = tokio::spawn(async move {
        for response in responses {
            let (mut socket, _) = listener.accept().await.unwrap();
            let (_, v) = request(&mut socket).await;
            capture.lock().await.push(v);
            reply(
                &mut socket,
                200,
                &envelope(&response).to_string(),
                "application/json",
            )
            .await;
        }
    });
    let temp = tempfile::tempdir().unwrap();
    let result = workflow::chain(temp.path(), &s).await.unwrap();
    task.await.unwrap();
    assert_eq!(seen.lock().await.len(), 3);
    assert_eq!(result["grade"]["score"], 0);
    assert_eq!(result["next_selection"][0], "chain-point");
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../docs/spikes/artifacts/mock-chain.json");
    std::fs::write(path, serde_json::to_vec_pretty(&result).unwrap()).unwrap();
}

#[tokio::test]
async fn small_context_long_document_map_reduce_and_checkpoint_restart() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let s = Snapshot {
        endpoint: format!("http://{}", listener.local_addr().unwrap()),
        ..Default::default()
    };
    let t = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let (_, input) = request(&mut socket).await;
            let content = input["messages"][1]["content"].as_str().unwrap();
            assert!(content.len() < 2662);
            let mut summary = String::new();
            if content.contains("活化能") {
                summary.push_str("K_BEGIN活化能降低；");
            }
            if content.contains("质量") {
                summary.push_str("K_MID质量和化学性质不变；");
            }
            if content.contains("平衡") {
                summary.push_str("K_END平衡常数不变。");
            }
            if summary.is_empty() {
                summary = "归档记录。".into();
            }
            reply(
                &mut socket,
                200,
                &envelope(&json!({"summary":summary}).to_string()).to_string(),
                "application/json",
            )
            .await;
        }
    });
    let temp = tempfile::tempdir().unwrap();
    let text = workflow::long_fixture();
    assert!(text.len() > 4096 * 10);
    let first = workflow::long_document(temp.path(), &s, &text)
        .await
        .unwrap();
    assert_eq!(first["covered_bytes"], text.len());
    assert!(first["reduce_layers"].as_u64().unwrap() > 0);
    assert!(first["distributed_facts_lexically_present"]
        .as_object()
        .unwrap()
        .values()
        .all(|v| v == true));
    let second = workflow::long_document(temp.path(), &s, &text)
        .await
        .unwrap();
    assert!(second["actual_calls"].as_u64().unwrap() < first["actual_calls"].as_u64().unwrap());
    let checkpoint = temp.path().join("map-0.json");
    let mut tampered: Value = serde_json::from_slice(&std::fs::read(&checkpoint).unwrap()).unwrap();
    tampered["summary"] = json!("TAMPERED");
    std::fs::write(checkpoint, serde_json::to_vec(&tampered).unwrap()).unwrap();
    let repaired = workflow::long_document(temp.path(), &s, &text)
        .await
        .unwrap();
    assert!(repaired["actual_calls"].as_u64().unwrap() > second["actual_calls"].as_u64().unwrap());
    t.abort();
    std::fs::write(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../docs/spikes/artifacts/mock-long-document.json"),
        serde_json::to_vec_pretty(&first).unwrap(),
    )
    .unwrap();
}

#[tokio::test]
async fn two_points_ten_days_rewrite_then_reuse_remains_answerable() {
    use learnloom_m0::store::Store;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let s = Snapshot {
        endpoint: format!("http://{}", listener.local_addr().unwrap()),
        ..Default::default()
    };
    let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = count.clone();
    let server = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let (_, wire) = request(&mut socket).await;
            seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let input: Value =
                serde_json::from_str(wire["messages"][1]["content"].as_str().unwrap()).unwrap();
            let point = input["point_id"].as_str().unwrap();
            let q = Question {
                point_id: point.into(),
                prompt: format!("{point}：催化剂如何作用？"),
                reference: "降低活化能，平衡不变。".into(),
                explanation: "改变路径。".into(),
                required: vec!["降低活化能".into(), "平衡不变".into()],
                critical_errors: vec!["改变平衡".into()],
            };
            reply(
                &mut socket,
                200,
                &envelope(&serde_json::to_string(&q).unwrap()).to_string(),
                "application/json",
            )
            .await;
        }
    });
    let temp = tempfile::tempdir().unwrap();
    let mut db = Store::open(temp.path()).unwrap();
    for point in ["point-a", "point-b"] {
        let w = Wiki {
            page_id: format!("page-{point}"),
            point_id: point.into(),
            title: point.into(),
            scope: point.into(),
            body: "催化剂降低活化能，平衡不变。".into(),
            evidence: "降低活化能".into(),
        };
        let sid = db.save_source(point, &w.body).unwrap();
        let e = db
            .enqueue(&format!("ingest:{point}"), "ingest", &w.page_id, point, &s)
            .unwrap();
        db.claim(&e).unwrap();
        let c = db.call(&e).unwrap();
        db.publish_wiki(&e, &c, &sid, &w, false).unwrap();
    }
    let mut history = vec![];
    for day in 1..=10 {
        let session = db.session(&format!("2026-10-{day:02}")).unwrap();
        let e = db
            .enqueue(
                &format!("daily:{session}"),
                "daily",
                &session,
                "two-points",
                &s,
            )
            .unwrap();
        db.bind_session(&session, &e).unwrap();
        db.claim(&e).unwrap();
        let c = db.call(&e).unwrap();
        let mut candidates = vec![];
        for point in ["point-a", "point-b"] {
            let (q, calls, repeated) = workflow::question_with_history(
                &s,
                point,
                "催化剂降低活化能，平衡不变。",
                &history,
            )
            .await
            .unwrap();
            assert_eq!(calls, if day == 1 { 1 } else { 2 });
            assert_eq!(repeated, day != 1);
            candidates.push(q);
        }
        let published = db.publish_questions(&e, &c, &session, &candidates).unwrap();
        assert_eq!(published.len(), 2);
        history.extend(candidates);
    }
    server.abort();
    assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 38);
    let reused: i64 = db
        .conn
        .query_row(
            "SELECT count(*) FROM questions WHERE reused_from IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(reused, 18);
}

#[tokio::test]
async fn queued_config_and_retry_use_actual_versioned_credentials() {
    use learnloom_m0::{config, store::Store};
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let a = format!("m0-key-a-{}", learnloom_m0::id());
    let b = format!("m0-key-b-{}", learnloom_m0::id());
    config::write_credential(&a, "synthetic-version-A").unwrap();
    config::write_credential(&b, "synthetic-version-B").unwrap();
    let server = tokio::spawn(async move {
        let mut records = vec![];
        for _ in 0..2 {
            let (mut socket, _) = listener.accept().await.unwrap();
            records.push(request(&mut socket).await);
            reply(
                &mut socket,
                200,
                &envelope("{\"ok\":true}").to_string(),
                "application/json",
            )
            .await;
        }
        records
    });
    let temp = tempfile::tempdir().unwrap();
    let mut db = Store::open(temp.path()).unwrap();
    let mut settings = config::Settings::default();
    settings.replace(&endpoint, "model-A", Some(a.clone()));
    let queued = db
        .enqueue("queued-model", "chat", "message", "test", &settings.current)
        .unwrap();
    settings.replace(&endpoint, "model-B", Some(b.clone()));
    settings.language("en");
    db.claim(&queued).unwrap();
    let snap = db.snapshot(&queued.run).unwrap();
    let r1 = model::generate(
        &snap,
        "JSON",
        &json!({}),
        Duration::from_secs(2),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(r1.config_revision, 2);
    db.stop(&queued, "failed").unwrap();
    let retry = db.retry(&queued, "new-model", &settings.current).unwrap();
    db.claim(&retry).unwrap();
    let r2 = model::generate(
        &db.snapshot(&retry.run).unwrap(),
        "JSON",
        &json!({}),
        Duration::from_secs(2),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(r2.config_revision, 3);
    let records = server.await.unwrap();
    assert!(records[0].0.contains("Bearer synthetic-version-A"));
    assert!(records[1].0.contains("Bearer synthetic-version-B"));
    assert_eq!(records[0].1["model"], "model-A");
    assert_eq!(records[1].1["model"], "model-B");
    config::credential(&a).unwrap().delete_credential().unwrap();
    config::credential(&b).unwrap().delete_credential().unwrap();
    assert_eq!(
        model::generate(
            &snap,
            "JSON",
            &json!({}),
            Duration::from_secs(1),
            CancellationToken::new()
        )
        .await
        .unwrap_err(),
        "CREDENTIAL_UNAVAILABLE"
    );
}
