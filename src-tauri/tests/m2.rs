use learnloom_m0::{app::AppCore, chat_run, config::Snapshot, knowledge::Library, retrieval};
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
    sync::Notify,
};
#[path = "../spikes/m2_fixture.rs"]
mod fixture;
struct Mock {
    url: String,
    count: Arc<AtomicUsize>,
    mode: Arc<AtomicUsize>,
    release: Arc<Notify>,
    requests: Arc<std::sync::Mutex<Vec<Value>>>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Mock {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn mock() -> Mock {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let count = Arc::new(AtomicUsize::new(0));
    let mode = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(Notify::new());
    let requests = Arc::new(std::sync::Mutex::new(vec![]));
    let (c, m, n, requests2) = (
        count.clone(),
        mode.clone(),
        release.clone(),
        requests.clone(),
    );
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = l.accept().await else {
                break;
            };
            let (c, m, n, requests) = (c.clone(), m.clone(), n.clone(), requests2.clone());
            tokio::spawn(async move {
                let mut b = vec![];
                let mut temp = [0; 8192];
                let req: Value = loop {
                    let size = s.read(&mut temp).await.unwrap_or(0);
                    if size == 0 {
                        return;
                    }
                    b.extend_from_slice(&temp[..size]);
                    if let Some(pos) = b.windows(4).position(|x| x == b"\r\n\r\n") {
                        let header = String::from_utf8_lossy(&b[..pos]);
                        let len = header
                            .lines()
                            .find_map(|l| {
                                l.to_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|x| x.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        if b.len() >= pos + 4 + len {
                            break serde_json::from_slice(&b[pos + 4..pos + 4 + len]).unwrap();
                        }
                    }
                };
                requests.lock().unwrap().push(req.clone());
                c.fetch_add(1, Ordering::SeqCst);
                let mode = m.load(Ordering::SeqCst);
                let input: Value =
                    serde_json::from_str(req["messages"][1]["content"].as_str().unwrap()).unwrap();
                if req["stream"] != true {
                    let out = json!({"query":"催化剂 活化能"});
                    let data=json!({"model":"mock","choices":[{"message":{"content":out.to_string()},"finish_reason":"stop"}],"usage":{"total_tokens":1}}).to_string();
                    let _=s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{data}",data.len()).as_bytes()).await;
                    return;
                }
                if mode == 4 {
                    let _ = s
                        .write_all(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n")
                        .await;
                    return;
                }
                let text = input["evidence"][0]["text"].as_str().unwrap_or("");
                let body = if mode == 2 {
                    format!("{} [E1]", text.repeat(60))
                } else {
                    format!("{} [E1]", text)
                };
                let out = if mode == 7 {
                    json!({"body":"库内未检索到直接依据。","answer_kind":"supplemented","references":[],"supplement":"可先从基本定义理解，再用具体例子检验。"})
                } else if mode == 5 {
                    json!({"body":"知识库依据不足。","answer_kind":"insufficient_context","references":[]})
                } else {
                    json!({"body":body,"answer_kind":"answered","references":[{"id":if mode==1{"E999"}else{"E1"},"quote":text}]})
                };
                if mode == 3 {
                    n.notified().await;
                }
                let _=s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n").await;
                let raw = out.to_string();
                let raw = if mode == 2 {
                    format!(
                        "{{\"body\":{}",
                        serde_json::to_string(&body).unwrap().trim_end_matches('"')
                    )
                } else {
                    raw
                };
                // JSON string boundaries, TCP fragmentation and Chinese UTF-8 all exercise the real gateway.
                let chars = raw.chars().collect::<Vec<_>>();
                for chunk in chars.chunks(19) {
                    let delta = chunk.iter().collect::<String>();
                    let event = format!(
                        "data: {}\n\n",
                        json!({"choices":[{"delta":{"content":delta},"finish_reason":null}]})
                    );
                    let _ = s.write_all(event.as_bytes()).await;
                }
                if mode == 2 {
                    return;
                }
                let event = format!(
                    "data: {}\n\ndata: [DONE]\n\n",
                    json!({"model":"mock","choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"total_tokens":1}})
                );
                let event = if mode == 6 {
                    event.replace("data: [DONE]\n\n", "")
                } else {
                    event
                };
                let _ = s.write_all(event.as_bytes()).await;
            });
        }
    });
    Mock {
        url,
        count,
        mode,
        release,
        requests,
        task,
    }
}
fn setup(p: &std::path::Path, url: &str) -> Arc<AppCore> {
    let app = AppCore::for_test(p).unwrap();
    let path = app.bootstrap().unwrap()["default_location"]["path"]
        .as_str()
        .unwrap()
        .to_owned();
    {
        let mut lib = Library::open(std::path::Path::new(&path), "M2", true).unwrap();
        fixture::seed(&mut lib).unwrap();
    }
    app.open_library("default", "M2", false).unwrap();
    app.seed_test_model(
        Snapshot {
            endpoint: url.into(),
            ..Default::default()
        },
        true,
    )
    .unwrap();
    app
}
async fn wait(app: &Arc<AppCore>, q: &Value) -> Value {
    for _ in 0..1500 {
        let s = app.job(q["job_id"].as_str().unwrap()).unwrap();
        if !["queued", "running"].contains(&s["status"].as_str().unwrap()) {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timeout");
}
fn ask(app: &Arc<AppCore>, cid: Option<&str>, key: &str) -> Value {
    app.ask(&json!({"text":"催化剂如何影响活化能？","conversation_id":cid,"request_key":key}))
        .unwrap()
}
#[tokio::test]
async fn stream_followup_idempotence_full_snapshot_and_pinned_sources() {
    let temp = tempfile::tempdir().unwrap();
    let mock = mock().await;
    let app = setup(temp.path(), &mock.url);
    let q = ask(&app, None, "once");
    let (subscription, _, _, token) = app
        .subscribe_chat(q["conversation_id"].as_str().unwrap())
        .unwrap();
    app.close_subscriptions();
    assert!(token.is_cancelled());
    app.unsubscribe_chat(&subscription);
    assert_eq!(ask(&app, None, "once")["message_id"], q["message_id"]);
    assert!(app
        .ask(&json!({"text":"different","request_key":"once"}))
        .is_err());
    let result = wait(&app, &q).await;
    assert_eq!(result["status"], "succeeded", "{result}");
    let m = app.message(q["message_id"].as_str().unwrap()).unwrap();
    assert_eq!(m.references.len(), 1);
    assert!(app
        .page(&m.references[0].page_id, Some(&m.references[0].revision_id))
        .unwrap()["body"]
        .as_str()
        .unwrap()
        .contains(&m.references[0].excerpt));
    let q2 = ask(&app, q["conversation_id"].as_str(), "follow");
    assert_eq!(wait(&app, &q2).await["status"], "succeeded");
    let history = app
        .conversation(q["conversation_id"].as_str().unwrap(), None, 1)
        .unwrap();
    assert_eq!(history["messages"].as_array().unwrap().len(), 2);
    assert!(history["next_cursor"].is_number());
    assert_eq!(mock.count.load(Ordering::SeqCst), 3);
    assert_eq!(
        app.conversations(None, 30).unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
#[tokio::test]
async fn forged_citations_partial_break_and_insufficient_are_distinct() {
    let temp = tempfile::tempdir().unwrap();
    let mock = mock().await;
    let app = setup(temp.path(), &mock.url);
    for mode in [1, 2, 4, 5, 6] {
        mock.mode.store(mode, Ordering::SeqCst);
        let q = ask(&app, None, &format!("mode{mode}"));
        let j = wait(&app, &q).await;
        let m = app.message(q["message_id"].as_str().unwrap()).unwrap();
        if mode == 5 {
            assert_eq!(j["status"], "succeeded");
            assert_eq!(m.answer_kind.as_deref(), Some("insufficient_context"));
        } else {
            assert_eq!(j["status"], "failed", "{j}");
            assert!(m.references.is_empty());
            if mode == 2 {
                assert!(!m.body.is_empty());
                assert_eq!(j["calls"], 1);
            }
            if mode == 4 {
                assert_eq!(j["calls"], 1);
            }
        }
    }
    mock.mode.store(7, Ordering::SeqCst);
    let before = mock.count.load(Ordering::SeqCst);
    let q = app
        .ask(&json!({"text":"火星外星人？","request_key":"absent"}))
        .unwrap();
    assert_eq!(wait(&app, &q).await["status"], "succeeded");
    assert_eq!(mock.count.load(Ordering::SeqCst), before + 1);
    assert_eq!(
        app.message(q["message_id"].as_str().unwrap())
            .unwrap()
            .answer_kind
            .as_deref(),
        Some("supplemented")
    );
}
#[tokio::test]
async fn cancel_old_response_then_retry_uses_latest_config_without_overwrite() {
    let temp = tempfile::tempdir().unwrap();
    let mock = mock().await;
    mock.mode.store(3, Ordering::SeqCst);
    let app = setup(temp.path(), &mock.url);
    let q = ask(&app, None, "cancel");
    while mock.count.load(Ordering::SeqCst) == 0 {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(app
        .ask(&json!({"conversation_id":q["conversation_id"],"text":"busy","request_key":"busy"}))
        .unwrap_err()
        .contains("BUSY"));
    app.cancel_job(&json!({"job_id":q["job_id"],"expected_run_id":q["run_id"]}))
        .unwrap();
    let stopped = app.message(q["message_id"].as_str().unwrap()).unwrap();
    assert_eq!(stopped.status, "cancelled");
    app.seed_test_model(
        Snapshot {
            endpoint: mock.url.clone(),
            config_revision: 2,
            model: "new-model".into(),
            ..Default::default()
        },
        true,
    )
    .unwrap();
    mock.mode.store(0, Ordering::SeqCst);
    let next = app
        .retry_job(
            &json!({"job_id":q["job_id"],"expected_run_id":q["run_id"],"request_key":"retry"}),
        )
        .unwrap();
    let result = wait(&app, &q).await;
    assert_eq!(result["status"], "succeeded", "{result}");
    assert_ne!(next["run_id"], q["run_id"]);
    assert_eq!(result["config_revision"], 2);
    mock.release.notify_waiters();
    tokio::time::sleep(Duration::from_millis(40)).await;
    let final_m = app.message(q["message_id"].as_str().unwrap()).unwrap();
    assert_eq!(final_m.status, "succeeded");
    assert!(final_m.sequence > stopped.sequence);
    assert_eq!(final_m.run_id.as_deref(), next["run_id"].as_str());
    assert_eq!(
        mock.requests.lock().unwrap().last().unwrap()["model"],
        "new-model"
    );
}
#[test]
fn retrieval_short_mixed_titles_body_pending_damage_and_fixed_recall() {
    let temp = tempfile::tempdir().unwrap();
    let mut lib = Library::open(temp.path(), "检索", true).unwrap();
    let ids = fixture::seed(&mut lib).unwrap();
    let f = fixture::fixture();
    let mut found = 0;
    for q in f["answerable"].as_array().unwrap() {
        let r = retrieval::search(&lib, q["query"].as_str().unwrap(), 5).unwrap();
        if r.items
            .iter()
            .any(|h| h.page_id == ids[q["page"].as_u64().unwrap() as usize])
        {
            found += 1;
        } else {
            eprintln!("MISS {}", q["query"]);
        }
    }
    assert!(found >= 26, "Recall@5={found}/30");
    lib.db.mark_index_pending(&ids[0]).unwrap();
    let r = retrieval::search(&lib, "催化剂", 5).unwrap();
    assert!(r.items.iter().all(|h| h.page_id != ids[0]));
    assert!(r.excluded_index_pending_count > 0);
    lib.recover().unwrap();
    assert_eq!(
        retrieval::search(&lib, "催化剂", 5).unwrap().items[0].page_id,
        ids[0]
    );
    lib.db.conn.execute("DELETE FROM page_fts", []).unwrap();
    assert_eq!(
        retrieval::search(&lib, "催化剂", 5).unwrap().status,
        "index_unavailable"
    );
}
#[test]
fn partial_json_unicode_and_untrusted_citations() {
    learnloom_m0::budget::Budget::new(4096)
        .unwrap()
        .check(chat_run::ANSWER_PROMPT, "{}", "", "")
        .unwrap();
    assert_eq!(
        chat_run::visible_body(r#"{"body":"中文\uD83D\uDE00"#).unwrap(),
        "中文😀"
    );
    assert_eq!(
        chat_run::visible_body(r#"{"body":"中\uD83D"#).unwrap(),
        "中"
    );
    assert!(chat_run::visible_body(r#"{"body":"\u中文"#).is_err());
    assert!(chat_run::validate(
        r#"{"body":"伪造 [E9]","answer_kind":"answered","references":[{"id":"E9","quote":"x"}]}"#,
        &[]
    )
    .is_err());
    assert!(chat_run::validate(
        r#"{"body":"不足 [E1]","answer_kind":"insufficient_context","references":[]}"#,
        &[]
    )
    .is_err());
}

// Hand-authored fixture: no saved provider response or acceptance metadata.
fn public_citation_fixture() -> (Value, Vec<chat_run::Evidence>) {
    let quote1="队列从队尾入队。";
    let quote2="队列从队首出队。";
    let evidence=serde_json::from_value(json!([{"id":"E1","hit":{
        "page_id":"fixture-page","revision_id":"fixture-revision","title":"队列",
        "chunk_id":1,"ordinal":0,"anchor":"chunk-0","heading":"队列",
        "excerpt":format!("{quote1}{quote2}"),"content_hash":learnloom_m0::hash(&format!("{quote1}{quote2}")),"score":1.0
    }}])).unwrap();
    let answer=json!({"body":"队列从队尾入队、从队首出队。[E1]","answer_kind":"answered",
        "references":[{"id":"E1","quote":quote1},{"id":"E1","quote":quote2}]});
    (answer,evidence)
}
#[test]
fn multiple_quotes_are_checked_then_normalized() {
    let (answer,evidence)=public_citation_fixture();
    let (_,kind,refs)=chat_run::validate(&answer.to_string(), &evidence).unwrap();
    assert_eq!(kind, "answered");
    assert_eq!(refs.len(), 1);
    assert!(evidence[0].hit.excerpt.contains(&refs[0].excerpt));
    let mut bad=answer;
    bad["references"][1]["quote"] = json!("不存在的第二段证据");
    assert!(chat_run::validate(&bad.to_string(), &evidence).is_err());
}

#[test]
fn supplement_is_separate_and_cannot_forge_library_citations() {
    let (mut answer,evidence)=public_citation_fixture();
    answer["answer_kind"] = json!("supplemented");
    answer["supplement"] = json!("额外常识示例，不冒充库内事实。");
    let (body,kind,refs) = chat_run::validate(&answer.to_string(), &evidence).unwrap();
    assert_eq!(kind,"supplemented");assert!(!refs.is_empty());assert!(body.contains("### AI 补充 / General knowledge"));
    answer["supplement"] = json!("伪造库内支持 [E1]");
    assert!(chat_run::validate(&answer.to_string(), &evidence).is_err());
    answer["supplement"] = json!("");
    assert!(chat_run::validate(&answer.to_string(), &evidence).is_err());
    let general=json!({"body":"库内没有直接依据。","answer_kind":"supplemented","references":[],"supplement":"通用解释。"});
    assert!(chat_run::validate(&general.to_string(), &[]).unwrap().2.is_empty());
    let mut bad=general.clone();bad["body"]=json!("假引用 [E9]");assert!(chat_run::validate(&bad.to_string(), &[]).is_err());
    bad=general;bad["answer_kind"]=json!("answered");assert!(chat_run::validate(&bad.to_string(), &[]).is_err());
}

#[test]
fn a_saved_citation_keeps_old_revision_after_new_page_publication() {
    use learnloom_m0::knowledge::{PreparedPage, PreparedPoint};
    let temp = tempfile::tempdir().unwrap();
    let mut lib = Library::open(temp.path(), "版本引用", true).unwrap();
    let ids = fixture::seed(&mut lib).unwrap();
    let hit = retrieval::search(&lib, "催化剂", 1)
        .unwrap()
        .items
        .remove(0);
    let old_rev = hit.revision_id.clone();
    let evidence = chat_run::Evidence {
        id: "E1".into(),
        hit: hit.clone(),
    };
    let body = format!("{}\n新增记录：来源仍保留催化剂的原始定义。", hit.excerpt);
    let sid = lib
        .save_source("新增记录", &body, "text", "new-version")
        .unwrap()["source_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let e = lib
        .db
        .enqueue(
            "update-version",
            "organize",
            &sid,
            &body,
            &Snapshot::default(),
        )
        .unwrap();
    lib.db.claim(&e).unwrap();
    let call = lib.db.call(&e).unwrap();
    lib.db
        .conn
        .execute(
            "INSERT INTO run_page_bases VALUES(?1,?2,?3)",
            rusqlite::params![e.run, ids[0], old_rev],
        )
        .unwrap();
    lib.publish(
        &e,
        &call,
        &sid,
        &[PreparedPage {
            id: ids[0].clone(),
            title: "催化剂".into(),
            scope: "M2固定资料".into(),
            definition: "催化剂".into(),
            body: body.clone(),
            base: Some(old_rev.clone()),
            points: vec![PreparedPoint {
                id: learnloom_m0::id(),
                name: "原定义".into(),
                definition: "原定义".into(),
                evidence: hit.excerpt.clone(),
            }],
            links: vec![],
        }],
        false,
    )
    .unwrap();
    let output=json!({"body":"降低活化能。[E1]","answer_kind":"answered","references":[{"id":"E1","quote":hit.excerpt}]}).to_string();
    let (_, _, refs) = chat_run::validate(&output, &[evidence.clone()]).unwrap();
    assert_eq!(refs[0].revision_id, old_rev);
    assert_ne!(
        retrieval::search(&lib, "催化剂", 1).unwrap().items[0].revision_id,
        old_rev
    );
    assert!(lib.page(&refs[0].page_id, Some(&old_rev)).unwrap()["body"]
        .as_str()
        .unwrap()
        .contains(&refs[0].excerpt));
    let invented=json!({"body":"有引用。[E1]","answer_kind":"answered","references":[{"id":"E1","quote":"新版本里不存在的证据"}]}).to_string();
    assert!(chat_run::validate(&invented, &[evidence]).is_err());
}

#[tokio::test]
async fn queued_request_keeps_acceptance_snapshot_and_old_response_before_retry_is_ignored() {
    let temp = tempfile::tempdir().unwrap();
    let mock = mock().await;
    mock.mode.store(3, Ordering::SeqCst);
    let app = setup(temp.path(), &mock.url);
    let first = ask(&app, None, "first");
    while mock.count.load(Ordering::SeqCst) == 0 {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let queued = ask(&app, None, "queued");
    app.seed_test_model(
        Snapshot {
            endpoint: mock.url.clone(),
            config_revision: 2,
            model: "changed".into(),
            ..Default::default()
        },
        true,
    )
    .unwrap();
    assert_eq!(
        app.job(queued["job_id"].as_str().unwrap()).unwrap()["config_revision"],
        1
    );
    app.cancel_job(&json!({"job_id":first["job_id"],"expected_run_id":first["run_id"]}))
        .unwrap();
    mock.mode.store(0, Ordering::SeqCst);
    mock.release.notify_waiters();
    assert_eq!(wait(&app, &queued).await["status"], "succeeded");
    assert_eq!(
        app.message(first["message_id"].as_str().unwrap())
            .unwrap()
            .status,
        "cancelled"
    );
    let reqs = mock.requests.lock().unwrap().clone();
    assert!(reqs.iter().all(|r| r["model"] == "deepseek-flash"));
    let new=app.retry_job(&json!({"job_id":first["job_id"],"expected_run_id":first["run_id"],"request_key":"after-old"})).unwrap();
    assert_eq!(wait(&app, &new).await["status"], "succeeded");
    assert_eq!(
        mock.requests.lock().unwrap().last().unwrap()["model"],
        "changed"
    );
}
#[tokio::test]
async fn gateway_timeout_and_missing_done_cannot_succeed() {
    let mock = mock().await;
    mock.mode.store(3, Ordering::SeqCst);
    let snap = Snapshot {
        endpoint: mock.url.clone(),
        ..Default::default()
    };
    let input = json!({"question":"test","evidence":[{"id":"E1","text":"原文"}]});
    let r = learnloom_m0::model::stream_chat(
        &snap,
        chat_run::ANSWER_PROMPT,
        &input,
        Duration::from_millis(80),
        tokio_util::sync::CancellationToken::new(),
        |_| async { Ok(()) },
    )
    .await;
    assert!(r.unwrap_err().starts_with("TIMEOUT"));
}
#[test]
fn restart_marks_queued_and_running_chat_interrupted_without_recharging() {
    use learnloom_m0::{chat, store::Execution};
    for running in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("恢复 中文 空格");
        let mut lib = Library::open(&path, "恢复", true).unwrap();
        let cid = learnloom_m0::id();
        let uid = learnloom_m0::id();
        let mid = learnloom_m0::id();
        let e: Execution = lib
            .db
            .enqueue(
                "chat-restart",
                "chat",
                &mid,
                "question",
                &Snapshot::default(),
            )
            .unwrap();
        lib.db
            .conn
            .execute_batch("BEGIN;PRAGMA defer_foreign_keys=ON;")
            .unwrap();
        lib.db
            .conn
            .execute(
                "INSERT INTO conversations VALUES(?1,'重启',0,0,NULL)",
                [&cid],
            )
            .unwrap();
        lib.db
            .conn
            .execute(
                "INSERT INTO messages VALUES(?1,?2,?3,'已保存部分正文','[]','queued',4)",
                rusqlite::params![mid, e.job, e.run],
            )
            .unwrap();
        lib.db
            .conn
            .execute(
                "INSERT INTO chat_turns VALUES(?1,?2,0,'question',0,?3)",
                rusqlite::params![uid, cid, mid],
            )
            .unwrap();
        lib.db
            .conn
            .execute(
                "INSERT INTO chat_meta VALUES(?1,?2,?3,NULL,0,0)",
                rusqlite::params![mid, cid, uid],
            )
            .unwrap();
        lib.db
            .conn
            .execute(
                "UPDATE conversations SET active_message_id=? WHERE id=?",
                [&mid, &cid],
            )
            .unwrap();
        lib.db
            .conn
            .execute("INSERT INTO job_meta VALUES(?1,'queued','',0,0)", [&e.job])
            .unwrap();
        lib.db.conn.execute_batch("COMMIT;").unwrap();
        if running {
            lib.db.claim(&e).unwrap();
        }
        drop(lib);
        let mut lib = Library::open(&path, "", false).unwrap();
        lib.recover().unwrap();
        let m = chat::message(&lib.db.conn, &mid).unwrap();
        assert_eq!(m.status, "interrupted");
        assert_eq!(m.body, "已保存部分正文");
        assert!(m.sequence > 4);
        assert_eq!(
            lib.db
                .conn
                .query_row("SELECT count(*) FROM model_calls", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert!(
            chat::conversation(&lib, &cid, None, 30).unwrap()["conversation"]["active_message_id"]
                .is_null()
        );
    }
}

#[test]
fn supplement_wrong_kind_and_placeholder_are_rejected() {
    let raw=r#"{"body":"没有依据 [E#]","answer_kind":"insufficient_context","references":[],"supplement":"这是独立的一般知识说明。"}"#;
    assert!(chat_run::validate(raw,&[]).unwrap_err().contains("supplement kind mismatch"));
    let mut reply:serde_json::Value=serde_json::from_str(raw).unwrap();
    reply["answer_kind"]=json!("supplemented");
    assert!(chat_run::validate(&reply.to_string(),&[]).unwrap_err().starts_with("INVALID_CITATION"));
    reply["body"]=json!("知识库暂无相关资料。");
    let (body,kind,refs)=chat_run::validate(&reply.to_string(),&[]).unwrap();
    assert_eq!(kind,"supplemented");assert!(refs.is_empty());
    assert!(body.contains("### AI 补充 / General knowledge"));
}
