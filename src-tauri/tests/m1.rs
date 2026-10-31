use learnloom_m0::{
    app::AppCore,
    config::Snapshot,
    id,
    knowledge::{Library, PreparedPage, PreparedPoint},
};
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
async fn mock() -> (
    String,
    Arc<AtomicUsize>,
    Arc<AtomicUsize>,
    tokio::task::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let count = Arc::new(AtomicUsize::new(0));
    let mode = Arc::new(AtomicUsize::new(0));
    let c = count.clone();
    let m = mode.clone();
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let c = c.clone();
            let m = m.clone();
            tokio::spawn(async move {
                let mut buffer = vec![];
                let mut temp = [0; 4096];
                let parsed = loop {
                    let n = socket.read(&mut temp).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buffer.extend_from_slice(&temp[..n]);
                    if let Some(pos) = buffer.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&buffer[..pos]);
                        let len = headers
                            .lines()
                            .find_map(|l| {
                                l.to_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|n| n.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        if buffer.len() >= pos + 4 + len {
                            break serde_json::from_slice::<Value>(&buffer[pos + 4..pos + 4 + len])
                                .unwrap();
                        }
                    }
                };
                c.fetch_add(1, Ordering::SeqCst);
                let mode = m.load(Ordering::SeqCst);
                if mode == 2 {
                    tokio::time::sleep(Duration::from_millis(350)).await;
                }
                let input: Value =
                    serde_json::from_str(parsed["messages"][1]["content"].as_str().unwrap())
                        .unwrap();
                let output = if input.get("test").is_some() {
                    assert!(parsed["max_tokens"].as_u64().unwrap() <= 1024);
                    json!({"ok":true})
                } else if input.get("topics").is_some() {
                    let raw = input["source"].as_str().unwrap();
                    let evidence = raw.chars().take(24).collect::<String>();
                    json!({"sections":[{"topic":0,"body":format!("整理后的资料：{evidence}"),"points":[{"name":"路径与平衡","definition":"催化剂的作用","evidence":if mode==3{"不存在的引用"}else{&evidence}}]}]})
                } else if input.get("existing_pages").is_some() {
                    let existing = input["existing_pages"].as_array().and_then(|v| v.first());
                    json!({"pages":[{"title":"催化剂","scope":"化学","definition":"催化剂与反应平衡","existing_page_id":existing.map(|v|v["id"].clone())}]})
                } else {
                    json!({"summary":"催化剂影响反应路径，平衡位置不变。"})
                };
                let status = if mode == 1 { 500 } else { 200 };
                let envelope=json!({"model":"mock-cloud","choices":[{"message":{"content":output.to_string()},"finish_reason":"stop"}],"usage":{"total_tokens":1}}).to_string();
                let response=format!("HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{envelope}",envelope.len());
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });
    (url, count, mode, task)
}
async fn wait(app: &Arc<AppCore>, job: &str) -> Value {
    for _ in 0..400 {
        let state = app.job(job).unwrap();
        if !["queued", "running"].contains(&state["status"].as_str().unwrap()) {
            return state;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("job timeout")
}
fn setup(profile: &std::path::Path, url: &str) -> Arc<AppCore> {
    let app = AppCore::for_test(profile).unwrap();
    app.open_library("default", "学习 库", true).unwrap();
    app.seed_test_model(
        Snapshot {
            endpoint: url.into(),
            model: "mock-cloud".into(),
            ..Default::default()
        },
        true,
    )
    .unwrap();
    app
}
#[test]
fn one_million_context_saves_and_survives_restart() {
    let temp = tempfile::tempdir().unwrap();
    let app = AppCore::for_test(temp.path()).unwrap();
    let request = json!({"endpoint":"https://example.com/v1","model":"large-context","key":"","context":1048576,"expected_config_revision":1});
    let saved = app.save_model(&request).unwrap();
    assert_eq!(saved["settings"]["context"], 1048576);
    let mut invalid = request.clone();
    invalid["expected_config_revision"] = saved["settings"]["config_revision"].clone();
    for value in [json!(2047), json!(1048577), json!(4096.5)] {
        invalid["context"] = value;
        assert_eq!(app.save_model(&invalid).unwrap_err(), "INVALID_CONTEXT_LIMIT");
    }
    drop(app);
    assert_eq!(AppCore::for_test(temp.path()).unwrap().bootstrap().unwrap()["settings"]["context"], 1048576);
    let budget = learnloom_m0::budget::Budget::new(1048576).unwrap();
    assert_eq!(budget.output, 2048);
    assert!(budget.check("Return JSON", &"x".repeat(200000), "", "").is_ok());
}
#[tokio::test]
async fn one_million_context_connection_probe_keeps_saved_capacity() {
    let temp = tempfile::tempdir().unwrap();
    let (url, count, _, server) = mock().await;
    let app = AppCore::for_test(temp.path()).unwrap();
    app.seed_test_model(Snapshot { endpoint:url, context:1048576, ..Default::default() }, false).unwrap();
    assert_eq!(app.check_connection(1).await.unwrap()["passed"], true);
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert_eq!(app.bootstrap().unwrap()["settings"]["context"], 1048576);
    server.abort();
}
#[tokio::test]
async fn source_to_wiki_and_restart_are_real_storage_with_mock_model() {
    let temp = tempfile::tempdir().unwrap();
    let (url, count, _, server) = mock().await;
    let profile = temp.path().join("配置 中文 空格");
    let app = setup(&profile, &url);
    let v = json!({"title":"催化剂","text":"催化剂降低活化能，不改变平衡位置。","request_key":"save-1"});
    let saved = app.save_source(&v).unwrap();
    let sid = saved["source_id"].as_str().unwrap();
    assert_eq!(app.save_source(&v).unwrap()["source_id"], sid);
    assert!(app
        .save_source(&json!({"title":"changed","text":"changed","request_key":"save-1"}))
        .is_err());
    let job = app.start_ingest(sid).unwrap();
    let same = app.start_ingest(sid).unwrap();
    assert_eq!(job, same);
    let result = wait(&app, job["job_id"].as_str().unwrap()).await;
    assert_eq!(result["status"], "succeeded", "{result}");
    assert_eq!(count.load(Ordering::SeqCst), 2);
    let state = app.bootstrap().unwrap();
    let pid = state["pages"][0]["id"].as_str().unwrap().to_owned();
    let root = state["library"]["path"].as_str().unwrap().to_owned();
    assert_eq!(app.page(&pid, None).unwrap()["sources"][0]["id"], sid);
    assert_eq!(app.search("活化能").unwrap()["items"][0]["page_id"], pid);
    drop(app);
    tokio::time::sleep(Duration::from_millis(30)).await;
    let app = AppCore::for_test(&profile).unwrap();
    assert_eq!(
        app.bootstrap().unwrap()["pages"].as_array().unwrap().len(),
        1
    );
    assert!(app.page(&pid, None).unwrap()["body"]
        .as_str()
        .unwrap()
        .contains("降低活化能"));
    assert!(Library::open(std::path::Path::new(&root), "", false).is_err());
    server.abort();
}
#[tokio::test]
async fn failed_retry_uses_new_config_and_cancelled_response_never_publishes() {
    let temp = tempfile::tempdir().unwrap();
    let (url, _, mode, server) = mock().await;
    let app = setup(temp.path(), &url);
    mode.store(1, Ordering::SeqCst);
    let sid = app
        .save_source(
            &json!({"title":"催化剂","text":"催化剂不改变平衡位置。","request_key":"source"}),
        )
        .unwrap()["source_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let job = app.start_ingest(&sid).unwrap();
    assert_eq!(
        wait(&app, job["job_id"].as_str().unwrap()).await["status"],
        "failed"
    );
    mode.store(2, Ordering::SeqCst);
    app.seed_test_model(
        Snapshot {
            endpoint: url.clone(),
            config_revision: 2,
            model: "new-model".into(),
            ..Default::default()
        },
        true,
    )
    .unwrap();
    let next = app
        .retry_job(
            &json!({"job_id":job["job_id"],"expected_run_id":job["run_id"],"request_key":"retry"}),
        )
        .unwrap();
    assert_eq!(
        app.job(job["job_id"].as_str().unwrap()).unwrap()["config_revision"],
        2
    );
    app.cancel_job(&json!({"job_id":next["job_id"],"expected_run_id":next["run_id"]}))
        .unwrap();
    tokio::time::sleep(Duration::from_millis(420)).await;
    assert!(app.bootstrap().unwrap()["pages"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(
        app.job(job["job_id"].as_str().unwrap()).unwrap()["status"],
        "cancelled"
    );
    mode.store(0, Ordering::SeqCst);
    let last=app.retry_job(&json!({"job_id":next["job_id"],"expected_run_id":next["run_id"],"request_key":"retry-new"})).unwrap();
    assert!(app
        .cancel_job(&json!({"job_id":next["job_id"],"expected_run_id":next["run_id"]}))
        .is_err());
    assert_eq!(
        wait(&app, last["job_id"].as_str().unwrap()).await["status"],
        "succeeded"
    );
    server.abort();
}
#[test]
fn unknown_directories_and_future_markers_are_protected() {
    let temp = tempfile::tempdir().unwrap();
    let unknown = temp.path().join("未知 内容");
    std::fs::create_dir(&unknown).unwrap();
    std::fs::write(unknown.join("notes.md"), "不要覆盖").unwrap();
    assert_eq!(
        learnloom_m0::knowledge::inspect(&unknown).unwrap()["kind"],
        "non_empty"
    );
    assert!(Library::open(&unknown, "x", true).is_err());
    assert_eq!(
        std::fs::read_to_string(unknown.join("notes.md")).unwrap(),
        "不要覆盖"
    );
    std::fs::write(
        unknown.join("learnloom.json"),
        json!({"format_version":99,"id":id(),"name":"future"}).to_string(),
    )
    .unwrap();
    assert!(learnloom_m0::knowledge::inspect(&unknown)
        .unwrap_err()
        .contains("UNSUPPORTED_KB_VERSION"));
}
#[test]
fn batch_publication_rolls_back_both_pages_and_indices() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("kb");
    let mut lib = Library::open(&path, "知识库", true).unwrap();
    let sid = lib
        .save_source("原文", "催化剂降低活化能。平衡位置不变。", "text", "save")
        .unwrap()["source_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let e = lib
        .db
        .enqueue("batch", "organize", &sid, "same", &Snapshot::default())
        .unwrap();
    lib.db.claim(&e).unwrap();
    let call = lib.db.call(&e).unwrap();
    let mut pages = vec![];
    for (title, evidence) in [("活化能", "催化剂降低活化能。"), ("平衡", "平衡位置不变。")]
    {
        let pid = id();
        lib.db
            .conn
            .execute(
                "INSERT INTO run_page_bases VALUES(?1,?2,NULL)",
                rusqlite::params![e.run, pid],
            )
            .unwrap();
        pages.push(PreparedPage {
            id: pid,
            title: title.into(),
            scope: "化学".into(),
            definition: title.into(),
            body: evidence.into(),
            base: None,
            points: vec![PreparedPoint {
                id: id(),
                name: title.into(),
                definition: title.into(),
                evidence: evidence.into(),
            }],
            links: vec![],
        });
    }
    assert!(lib.publish(&e, &call, &sid, &pages, true).is_err());
    assert_eq!(
        lib.db
            .conn
            .query_row("SELECT count(*) FROM pages", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        lib.db
            .conn
            .query_row("SELECT count(*) FROM page_fts", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert!(lib.publish(&e, &call, &sid, &pages, false).unwrap());
    // A stale repair target is superseded and repaired in this same recovery pass.
    lib.db.mark_index_pending(&pages[0].id).unwrap();
    lib.db
        .conn
        .execute(
            "UPDATE index_tasks SET target='obsolete-revision' WHERE page_id=?",
            [&pages[0].id],
        )
        .unwrap();
    assert!(lib.search("活化能").unwrap().is_empty());
    lib.recover().unwrap();
    assert_eq!(
        lib.page(&pages[0].id, None).unwrap()["index_state"],
        "ready"
    );
    assert_eq!(lib.search("活化能").unwrap(), vec![pages[0].id.clone()]);
    assert_eq!(
        lib.db
            .conn
            .query_row("SELECT count(*) FROM pages", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
}
#[test]
fn production_prompts_fit_the_verified_4k_system_budget() {
    use learnloom_m0::{
        budget::Budget,
        ingest::{MAP_PROMPT, PLAN_PROMPT, REDUCE_PROMPT, SECTION_PROMPT},
    };
    let budget = Budget::new(4096).unwrap();
    for prompt in [MAP_PROMPT, REDUCE_PROMPT, PLAN_PROMPT, SECTION_PROMPT] {
        budget.check(prompt, "{}", "", "").unwrap();
    }
}
#[tokio::test]
async fn long_source_checkpoints_are_verified_and_unhandled_chunks_fail_explicitly() {
    let temp = tempfile::tempdir().unwrap();
    let (url, count, _, server) = mock().await;
    let app = setup(temp.path(), &url);
    let body = "催化剂降低活化能，不改变平衡位置。".repeat(40);
    let sid = app
        .save_source(&json!({"title":"催化剂长资料","text":body,"request_key":"long"}))
        .unwrap()["source_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let e = app.start_ingest(&sid).unwrap();
    let state = wait(&app, e["job_id"].as_str().unwrap()).await;
    assert_eq!(state["status"], "succeeded", "{state}");
    assert!(count.load(Ordering::SeqCst) > 3);
    let root = app.bootstrap().unwrap()["library"]["path"]
        .as_str()
        .unwrap()
        .to_string();
    let db = rusqlite::Connection::open(
        std::path::Path::new(&root).join(".learnloom/learnloom.sqlite3"),
    )
    .unwrap();
    assert!(
        db.query_row("SELECT count(*) FROM checkpoints", [], |r| r
            .get::<_, i64>(0))
            .unwrap()
            > 2
    );
    server.abort();
}

#[test]
fn setup_defer_import_permissions_utf8_and_atomic_settings() {
    let temp = tempfile::tempdir().unwrap();
    let profile = temp.path().join("设置 恢复");
    let app = AppCore::for_test(&profile).unwrap();
    app.setup_action("defer").unwrap();
    app.open_library("default", "中文 库", true).unwrap();
    assert_eq!(
        app.inspect_location("default").unwrap()["kind"],
        "learnloom"
    );
    assert_eq!(
        app.open_library("default", "", false).unwrap()["already_open"],
        true
    );
    let good = temp.path().join("资料 中文.md");
    std::fs::write(&good, "\u{feff}# 催化剂\n降低活化能。").unwrap();
    assert!(app
        .import_file(&json!({"token":"not-granted","request_key":"x"}))
        .is_err());
    let grant = app.grant_file(&good).unwrap();
    let imported = app
        .import_file(&json!({"token":grant["token"],"request_key":"import"}))
        .unwrap();
    assert_eq!(
        app.import_file(&json!({"token":grant["token"],"request_key":"import2"}))
            .unwrap()["source_id"],
        imported["source_id"]
    );
    assert!(
        app.source(imported["source_id"].as_str().unwrap()).unwrap()["body"]
            .as_str()
            .unwrap()
            .starts_with('#')
    );
    let bad = temp.path().join("bad.txt");
    std::fs::write(&bad, [0xff, 0xfe, 0x01]).unwrap();
    let grant = app.grant_file(&bad).unwrap();
    assert!(app
        .import_file(&json!({"token":grant["token"],"request_key":"bad"}))
        .unwrap_err()
        .starts_with("INVALID_UTF8"));
    assert!(app.save_model(&json!({"endpoint":"http://localhost:11434","model":"local","key":"","context":4096,"expected_config_revision":1})).is_err());
    app.save_language(&json!({"language":"en","expected_preferences_revision":1}))
        .unwrap();
    assert!(app
        .save_language(&json!({"language":"zh-CN","expected_preferences_revision":1}))
        .unwrap_err()
        .starts_with("CONFLICT"));
    let settings = profile.join("settings.json");
    let saved = std::fs::read(&settings).unwrap();
    std::fs::remove_file(&settings).unwrap();
    std::fs::create_dir(&settings).unwrap();
    assert!(app
        .save_language(&json!({"language":"zh-CN","expected_preferences_revision":2}))
        .is_err());
    assert_eq!(app.bootstrap().unwrap()["settings"]["language"], "en");
    std::fs::remove_dir(&settings).unwrap();
    std::fs::write(&settings, saved).unwrap();
    drop(app);
    let reopened = AppCore::for_test(&profile).unwrap();
    let state = reopened.bootstrap().unwrap();
    assert_eq!(state["setup"]["deferred"], true);
    assert_eq!(state["setup"]["step"], 1);
    assert_eq!(state["settings"]["language"], "en");
    assert_eq!(state["sources"].as_array().unwrap().len(), 1);
}
#[tokio::test]
async fn old_connection_cannot_override_new_revision_and_language_preserves_check() {
    let temp = tempfile::tempdir().unwrap();
    let (url, count, mode, server) = mock().await;
    let app = setup(temp.path(), &url);
    mode.store(2, Ordering::SeqCst);
    let old = app.clone();
    let check = tokio::spawn(async move { old.check_connection(1).await.unwrap() });
    while count.load(Ordering::SeqCst) == 0 {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    app.seed_test_model(
        Snapshot {
            endpoint: url.clone(),
            config_revision: 2,
            ..Default::default()
        },
        false,
    )
    .unwrap();
    let result = check.await.unwrap();
    assert_eq!(result["applied"], false);
    assert_eq!(
        app.bootstrap().unwrap()["settings"]["check_state"],
        "untested"
    );
    mode.store(0, Ordering::SeqCst);
    assert_eq!(app.check_connection(2).await.unwrap()["passed"], true);
    app.save_language(&json!({"language":"en","expected_preferences_revision":1}))
        .unwrap();
    assert_eq!(
        app.bootstrap().unwrap()["settings"]["check_state"],
        "passed"
    );
    app.setup_action("complete").unwrap();
    server.abort();
}
#[tokio::test]
async fn repeated_source_preserves_page_point_identity_and_original_content() {
    let temp = tempfile::tempdir().unwrap();
    let (url, _, _, server) = mock().await;
    let app = setup(temp.path(), &url);
    let mut pid = String::new();
    let mut point = String::new();
    let mut revisions = vec![];
    for (key, text) in [
        ("one", "催化剂降低活化能，不改变平衡位置。"),
        ("two", "催化剂降低活化能，不改变平衡位置。温度保持不变。"),
    ] {
        let s = app
            .save_source(&json!({"title":if key=="two"{"课堂记录 第二课"}else{"催化剂"},"text":text,"request_key":key}))
            .unwrap();
        let j = app
            .dispatch("start_ingest", json!({"source_id":s["source_id"]}))
            .await
            .unwrap();
        assert_eq!(
            wait(&app, j["job_id"].as_str().unwrap()).await["status"],
            "succeeded"
        );
        let state = app.bootstrap().unwrap();
        assert_eq!(state["pages"].as_array().unwrap().len(), 1);
        let page = app
            .page(state["pages"][0]["id"].as_str().unwrap(), None)
            .unwrap();
        if pid.is_empty() {
            pid = page["id"].as_str().unwrap().into();
            point = page["points"][0]["id"].as_str().unwrap().into();
        }
        assert_eq!(page["id"], pid);
        assert_eq!(page["points"][0]["id"], point);
        assert!(page["body"].as_str().unwrap().contains("不改变平衡位置"));
        revisions.push(page["revision"].clone());
    }
    assert_ne!(revisions[0], revisions[1]);
    assert!(app
        .page(&pid, Some(revisions[0].as_str().unwrap()))
        .unwrap()["body"]
        .as_str()
        .unwrap()
        .contains("催化剂"));
    assert_eq!(
        app.page(&pid, None).unwrap()["sources"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    server.abort();
}
#[tokio::test]
async fn invalid_evidence_and_source_limit_retain_input_without_publication() {
    let temp = tempfile::tempdir().unwrap();
    let (url, _, mode, server) = mock().await;
    let app = setup(temp.path(), &url);
    mode.store(3, Ordering::SeqCst);
    for (key, text, error) in [
        ("bad", "催化剂不改变平衡位置。".into(), "INVALID_OUTPUT"),
        ("oversized", "资料".repeat(25000), "SOURCE_TOO_LARGE"),
    ] {
        let s = app
            .save_source(&json!({"title":"资料","text":text,"request_key":key}))
            .unwrap();
        let j = app.start_ingest(s["source_id"].as_str().unwrap()).unwrap();
        let result = wait(&app, j["job_id"].as_str().unwrap()).await;
        assert_eq!(result["status"], "failed");
        assert!(
            result["error"].as_str().unwrap().starts_with(error),
            "{result}"
        );
        assert_eq!(
            app.source(s["source_id"].as_str().unwrap()).unwrap()["body"],
            text
        );
        assert!(app.bootstrap().unwrap()["pages"]
            .as_array()
            .unwrap()
            .is_empty());
    }
    server.abort();
}
#[tokio::test]
async fn missing_current_body_is_excluded_and_owned_orphans_are_cleaned() {
    let temp = tempfile::tempdir().unwrap();
    let (url, _, _, server) = mock().await;
    let profile = temp.path().join("profile");
    let app = setup(&profile, &url);
    let s = app
        .save_source(&json!({"title":"催化剂","text":"催化剂降低活化能。","request_key":"saved"}))
        .unwrap();
    let j = app.start_ingest(s["source_id"].as_str().unwrap()).unwrap();
    assert_eq!(
        wait(&app, j["job_id"].as_str().unwrap()).await["status"],
        "succeeded"
    );
    let state = app.bootstrap().unwrap();
    let pid = state["pages"][0]["id"].as_str().unwrap().to_string();
    let rev = state["pages"][0]["revision"].as_str().unwrap();
    let root = std::path::PathBuf::from(state["library"]["path"].as_str().unwrap());
    let body = root.join(format!("wiki/{rev}.md"));
    std::fs::remove_file(body).unwrap();
    let orphan = root.join(format!("wiki/{}.md", id()));
    std::fs::write(&orphan, "unpublished").unwrap();
    let user = root.join("wiki/用户文件.md");
    std::fs::write(&user, "保留").unwrap();
    drop(app);
    tokio::time::sleep(Duration::from_millis(30)).await;
    let app = AppCore::for_test(&profile).unwrap();
    assert!(app.search("活化能").unwrap()["items"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(app.page(&pid, None).is_err());
    assert!(!orphan.exists());
    assert!(user.exists());
    server.abort();
}

#[tokio::test]
async fn queued_run_keeps_snapshot_and_running_run_recovers_as_interrupted() {
    let temp = tempfile::tempdir().unwrap();
    let (url, _, _, server) = mock().await;
    let profile = temp.path().join("profile");
    let app = setup(&profile, &url);
    let state = app.bootstrap().unwrap();
    let root = std::path::PathBuf::from(state["library"]["path"].as_str().unwrap());
    drop(app);
    let (queued, running);
    {
        let mut lib = Library::open(&root, "", false).unwrap();
        let sid = lib
            .save_source("排队", "催化剂降低活化能。", "text", "s1")
            .unwrap()["source_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let sid2 = lib
            .save_source("中断", "催化剂不改变平衡位置。", "text", "s2")
            .unwrap()["source_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let snapshot = Snapshot {
            endpoint: url.clone(),
            model: "old-model".into(),
            ..Default::default()
        };
        queued = lib
            .db
            .enqueue("q", "organize", &sid, "hash", &snapshot)
            .unwrap();
        running = lib
            .db
            .enqueue("r", "organize", &sid2, "hash", &snapshot)
            .unwrap();
        lib.db.claim(&running).unwrap();
        for e in [&queued, &running] {
            lib.db
                .conn
                .execute("INSERT INTO job_meta VALUES(?1,'queued','',1,1)", [&e.job])
                .unwrap();
        }
    }
    let app = AppCore::for_test(&profile).unwrap();
    assert_eq!(app.job(&running.job).unwrap()["status"], "interrupted");
    app.seed_test_model(
        Snapshot {
            endpoint: url.clone(),
            model: "new-model".into(),
            config_revision: 2,
            ..Default::default()
        },
        true,
    )
    .unwrap();
    app.resume_queued().unwrap();
    assert_eq!(wait(&app, &queued.job).await["status"], "succeeded");
    assert_eq!(app.job(&queued.job).unwrap()["model"], "old-model");
    let db = rusqlite::Connection::open(root.join(".learnloom/learnloom.sqlite3")).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT config_revision FROM model_calls WHERE run_id=? LIMIT 1",
            [&queued.run],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    let retry = app
        .retry_job(
            &json!({"job_id":running.job,"expected_run_id":running.run,"request_key":"retry"}),
        )
        .unwrap();
    assert_eq!(wait(&app, &running.job).await["status"], "succeeded");
    assert_eq!(app.job(&running.job).unwrap()["model"], "new-model");
    assert_ne!(retry["run_id"], running.run);
    server.abort();
}
// An existing marker with a missing database is not an empty new knowledge base.
#[test]
fn missing_database_never_reinitializes_or_deletes_existing_content() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("kb");
    let lib = Library::open(&root, "原库", true).unwrap();
    drop(lib);
    let body = root.join(format!("wiki/{}.md", id()));
    std::fs::write(&body, "不可丢失的页面").unwrap();
    std::fs::remove_file(root.join(".learnloom/learnloom.sqlite3")).unwrap();
    let err = Library::open(&root, "", false).err().unwrap();
    assert!(err.starts_with("KB_DATABASE_MISSING"));
    assert_eq!(std::fs::read_to_string(body).unwrap(), "不可丢失的页面");
    assert!(!root.join(".learnloom/learnloom.sqlite3").exists());
}
