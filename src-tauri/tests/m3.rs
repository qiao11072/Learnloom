use learnloom_m0::{
    app::AppCore,
    config::Snapshot,
    knowledge::Library,
    learning::{Clock, Day},
    Result,
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
#[path = "../spikes/m2_fixture.rs"]
mod fixture;
#[path = "support/m4.rs"]
mod m4;
struct Fixed(i64);
impl Clock for Fixed {
    fn day(&self) -> Result<Day> {
        Ok(Day {
            date: format!("2026-10-{:02}", self.0),
            timezone: "Asia/Shanghai".into(),
            offset_minutes: 480,
            utc_ms: self.0 * 86400000,
        })
    }
}
fn setup(root: &std::path::Path, url: &str) -> Arc<AppCore> {
    setup_n(root, url, 1)
}
fn setup_n(root: &std::path::Path, url: &str, n: usize) -> Arc<AppCore> {
    let app = AppCore::for_test(root).unwrap();
    let b = app.bootstrap().unwrap();
    let path = b["default_location"]["path"].as_str().unwrap();
    {
        let mut lib = Library::open(std::path::Path::new(path), "M3", true).unwrap();
        fixture::seed(&mut lib).unwrap();
        lib.db.conn.execute("DELETE FROM point_meta WHERE point_id NOT IN(SELECT id FROM points ORDER BY id LIMIT ?)",[n]).unwrap();
        lib.db.conn.execute("DELETE FROM point_versions WHERE point_id NOT IN(SELECT id FROM points ORDER BY id LIMIT ?)",[n]).unwrap();
        lib.db
            .conn
            .execute(
                "DELETE FROM points WHERE id NOT IN(SELECT id FROM points ORDER BY id LIMIT ?)",
                [n],
            )
            .unwrap();
    }
    app.open_library("default", "M3", false).unwrap();
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
async fn wait(app: &Arc<AppCore>, v: &Value) -> Value {
    for _ in 0..1000 {
        let j = app.job(v["job_id"].as_str().unwrap()).unwrap();
        if !["queued", "running"].contains(&j["status"].as_str().unwrap()) {
            if j["status"] != "succeeded" {
                eprintln!("JOB {j}");
            }
            return j;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timeout")
}
async fn mock() -> (
    String,
    Arc<AtomicUsize>,
    Arc<AtomicUsize>,
    tokio::task::JoinHandle<()>,
) {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let mode = Arc::new(AtomicUsize::new(0));
    let calls = Arc::new(AtomicUsize::new(0));
    let (m, c) = (mode.clone(), calls.clone());
    let task = tokio::spawn(async move {
        loop {
            let (mut s, _) = l.accept().await.unwrap();
            let (m, c) = (m.clone(), c.clone());
            tokio::spawn(async move {
                let mut b = vec![];
                let mut temp = [0; 8192];
                let req: Value = loop {
                    let n = s.read(&mut temp).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    b.extend_from_slice(&temp[..n]);
                    if let Some(p) = b.windows(4).position(|v| v == b"\r\n\r\n") {
                        let h = String::from_utf8_lossy(&b[..p]);
                        let len = h
                            .lines()
                            .find_map(|v| {
                                v.to_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|n| n.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        if b.len() >= p + 4 + len {
                            break serde_json::from_slice(&b[p + 4..p + 4 + len]).unwrap();
                        }
                    }
                };
                let call_number = c.fetch_add(1, Ordering::SeqCst) + 1;
                let input: Value =
                    serde_json::from_str(req["messages"][1]["content"].as_str().unwrap()).unwrap();
                let mode = m.load(Ordering::SeqCst);
                if mode == 3 {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                }
                let out = if mode == 4 && call_number <= 2 {
                    json!({"invalid":true})
                } else if mode == 5 {
                    json!({"invalid":true})
                } else if input.get("answer").is_some() {
                    let a = input["answer"].as_str().unwrap();
                    if mode == 1 {
                        json!({"score":0})
                    } else {
                        let (matched, missing, critical) = if a == "yes" {
                            (
                                json!([{"id":"R0","quote":"yes"},{"id":"R1","quote":"yes"}]),
                                json!([]),
                                json!([]),
                            )
                        } else if a == "half" {
                            (
                                json!([{"id":"R0","quote":"half"}]),
                                json!(["R1"]),
                                json!([]),
                            )
                        } else {
                            (json!([]), json!(["R0", "R1"]), json!([]))
                        };
                        json!({"explanation":"mock semantic assessment","matched":matched,"missing":missing,"critical":critical,"noncritical_errors":[]})
                    }
                } else if input.get("candidate").is_some() {
                    json!({"valid":true,"reason":"mock check","distinct_items":true,"critical_are_false":true,"explanation_teaches":true,"explanation_support_quotes":[input["evidence"]],"support_quotes":[input["evidence"].as_str().unwrap().chars().take(2).collect::<String>(),input["evidence"].as_str().unwrap().chars().skip(2).collect::<String>()]})
                } else {
                    json!({"point_id":input["point_id"],"prompt":format!("Explain {} and its boundary",input["point_id"]),"reference":input["evidence"],"explanation":format!("{} 这两个要点需要分别判断，遗漏其一不能视为完整回答。",input["evidence"].as_str().unwrap_or("")),"required":["要点一","要点二"],"critical_errors":["错误命题"]})
                };
                let data=json!({"model":"mock","choices":[{"message":{"content":out.to_string()},"finish_reason":"stop"}],"usage":{"total_tokens":1}}).to_string();
                let _=s.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{data}",data.len()).as_bytes()).await;
            });
        }
    });
    (url, mode, calls, task)
}
#[tokio::test]
async fn single_point_slice_daily_recovery_and_three_scores() {
    let t = tempfile::tempdir().unwrap();
    let (url, mode, calls, task) = mock().await;
    let app = setup(t.path(), &url);
    let s = app.daily_with_clock(&Fixed(1)).unwrap();
    assert_eq!(wait(&app, &s).await["status"], "succeeded");
    let sid = s["id"].as_str().unwrap();
    let s = app.review_session(sid).unwrap();
    assert_eq!(s["actual_count"], 1);
    let q = &s["questions"][0];
    assert!(q.get("reference").is_none());
    let qid = q["id"].as_str().unwrap();
    assert_eq!(
        app.save_draft(&json!({"question_id":qid,"answer":"yes","revision":0}))
            .unwrap()["revision"],
        1
    );
    assert!(app
        .save_draft(&json!({"question_id":qid,"answer":"different","revision":0}))
        .is_err());
    let v = json!({"question_id":qid,"answer":"yes","submission_key":"first"});
    let a = app.submit_review(&v).unwrap();
    assert_eq!(app.submit_review(&v).unwrap(), a);
    assert_eq!(wait(&app, &a).await["status"], "succeeded");
    let s = app.review_session(sid).unwrap();
    assert_eq!(s["questions"][0]["attempt"]["score"], 2);
    assert_eq!(app.daily_with_clock(&Fixed(1)).unwrap()["id"], sid);
    assert!(app
        .submit_review(&json!({"question_id":qid,"answer":"changed","submission_key":"second"}))
        .is_err());
    for (day, answer, score) in [(2, "half", 1), (3, "wrong", 0)] {
        let s = app.daily_with_clock(&Fixed(day)).unwrap();
        assert_eq!(wait(&app, &s).await["status"], "succeeded");
        let s = app.review_session(s["id"].as_str().unwrap()).unwrap();
        assert!(!s["questions"][0]["reused_from"].is_null());
        let a=app.submit_review(&json!({"question_id":s["questions"][0]["id"],"answer":answer,"submission_key":format!("day{day}")})).unwrap();
        assert_eq!(wait(&app, &a).await["status"], "succeeded");
        assert_eq!(
            app.review_session(s["id"].as_str().unwrap()).unwrap()["questions"][0]["attempt"]
                ["score"],
            score
        );
    }
    let s = app.daily_with_clock(&Fixed(4)).unwrap();
    wait(&app, &s).await;
    let s = app.review_session(s["id"].as_str().unwrap()).unwrap();
    mode.store(1, Ordering::SeqCst);
    let a=app.submit_review(&json!({"question_id":s["questions"][0]["id"],"answer":"yes","submission_key":"failure"})).unwrap();
    assert_eq!(wait(&app, &a).await["status"], "failed");
    let failed = app.review_session(s["id"].as_str().unwrap()).unwrap();
    assert!(failed["questions"][0]["attempt"]["score"].is_null());
    let at = failed["questions"][0]["attempt"]["submitted_at"].clone();
    mode.store(0, Ordering::SeqCst);
    let retry = app
        .retry_job(
            &json!({"job_id":a["job_id"],"expected_run_id":a["run_id"],"request_key":"retry"}),
        )
        .unwrap();
    assert_eq!(wait(&app, &retry).await["status"], "succeeded");
    assert_eq!(
        app.review_session(s["id"].as_str().unwrap()).unwrap()["questions"][0]["attempt"]
            ["submitted_at"],
        at
    );
    let before = calls.load(Ordering::SeqCst);
    let released = Arc::downgrade(&app);
    drop(app);
    for _ in 0..200 {
        if released.strong_count()==0 {break;}
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let app = AppCore::for_test(t.path()).unwrap();
    assert_eq!(
        app.review_session(sid).unwrap()["questions"][0]["attempt"]["score"],
        2
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(calls.load(Ordering::SeqCst), before);
    task.abort();
}

#[tokio::test]
async fn concurrent_daily_cross_day_clock_rollback_cancel_retry_and_draft_restore() {
    let t = tempfile::tempdir().unwrap();
    let (url, mode, calls, task) = mock().await;
    let app = setup(t.path(), &url);
    let a = app.clone();
    let b = app.clone();
    let (s1, s2) = tokio::join!(
        tokio::task::spawn_blocking(move || a.daily_with_clock(&Fixed(8))),
        tokio::task::spawn_blocking(move || b.daily_with_clock(&Fixed(8)))
    );
    let s = s1.unwrap().unwrap();
    assert_eq!(s["id"], s2.unwrap().unwrap()["id"]);
    assert_eq!(wait(&app, &s).await["status"], "succeeded");
    let sid = s["id"].as_str().unwrap().to_string();
    let s = app.review_session(&sid).unwrap();
    let q = s["questions"][0]["id"].as_str().unwrap().to_string();
    app.save_draft(&json!({"question_id":q,"answer":"unfinished draft","revision":0}))
        .unwrap();
    let next = app.daily_with_clock(&Fixed(9)).unwrap();
    assert_ne!(next["id"], sid);
    wait(&app, &next).await;
    assert_eq!(app.daily_with_clock(&Fixed(8)).unwrap()["id"], sid);
    assert_eq!(
        app.review_list().unwrap()["items"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    mode.store(3, Ordering::SeqCst);
    let a = app
        .submit_review(&json!({"question_id":q,"answer":"yes","submission_key":"cancel"}))
        .unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    app.cancel_job(&json!({"job_id":a["job_id"],"expected_run_id":a["run_id"]}))
        .unwrap();
    let at = app.review_session(&sid).unwrap()["questions"][0]["attempt"]["submitted_at"].clone();
    mode.store(0, Ordering::SeqCst);
    let r = app
        .retry_job(
            &json!({"job_id":a["job_id"],"expected_run_id":a["run_id"],"request_key":"newrun"}),
        )
        .unwrap();
    assert_ne!(a["run_id"], r["run_id"]);
    assert_eq!(wait(&app, &r).await["status"], "succeeded");
    assert!(app
        .cancel_job(&json!({"job_id":a["job_id"],"expected_run_id":a["run_id"]}))
        .is_err());
    tokio::time::sleep(Duration::from_millis(350)).await;
    let s = app.review_session(&sid).unwrap();
    assert_eq!(s["questions"][0]["attempt"]["score"], 2);
    assert_eq!(s["questions"][0]["attempt"]["submitted_at"], at);
    let next = app.review_session(next["id"].as_str().unwrap()).unwrap();
    let q2 = &next["questions"][0]["id"];
    app.save_draft(&json!({"question_id":q2,"answer":"survives restart","revision":0}))
        .unwrap();
    let before = calls.load(Ordering::SeqCst);
    drop(app);
    let app = AppCore::for_test(t.path()).unwrap();
    assert_eq!(
        app.review_session(next["id"].as_str().unwrap()).unwrap()["questions"][0]["draft"],
        "survives restart"
    );
    assert!(!app.bootstrap().unwrap()["unfinished_review_sessions"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(calls.load(Ordering::SeqCst), before);
    task.abort();
}

#[test]
fn quotation_context_rejects_keyword_without_negation() {
    use learnloom_m0::{learning::contextual_grade, store::Question};
    let q = Question {
        point_id: "p".into(),
        prompt: "题目".into(),
        reference: "参考".into(),
        explanation: "解析".into(),
        required: vec!["降低活化能".into(), "不改变平衡".into()],
        critical_errors: vec!["改变平衡".into()],
    };
    let raw = json!({"explanation":"bad quote","matched":[{"id":"R0","quote":"降低活化能"}],"missing":["R1"],"critical":[],"noncritical_errors":[]});
    assert!(contextual_grade(&raw, &q, "我不认为降低活化能是催化剂的作用。").is_err());
    let raw = json!({"explanation":"完整语境没有命中","matched":[],"missing":["R0","R1"],"critical":[],"noncritical_errors":[]});
    assert_eq!(
        contextual_grade(&raw, &q, "我不认为降低活化能是催化剂的作用。")
            .unwrap()
            .score,
        0
    );
}

#[test]
fn explanation_must_teach_and_have_evidence_not_just_different_wording() {
    use learnloom_m0::{explanation, store::Question};
    let evidence = "密度等于质量除以体积，ρ=m/V。计算密度需要质量和体积。";
    let mut q = Question {
        point_id: "p".into(),
        prompt: "如何计算密度？".into(),
        reference: evidence.into(),
        explanation: evidence.into(),
        required: vec!["质量和体积".into(), "ρ=m/V".into()],
        critical_errors: vec![],
    };
    assert!(explanation::distinct(&q).is_err());
    q.explanation = format!(" {} ", evidence.replace('。', "！"));
    assert!(explanation::distinct(&q).is_err());
    q.explanation = "密度就是质量与体积的比值，需要质量和体积两个量。".into();
    let rejected = json!({"valid":true,"reason":"mere paraphrase","explanation_teaches":false,"explanation_support_quotes":[evidence]});
    assert!(explanation::validate_review(&rejected, &q, evidence).is_err());
    q.explanation="先确定质量和体积，再把质量作为被除数、体积作为除数。交换除法顺序得到的是体积与质量的比值，与密度定义不符。".into();
    let checked = json!({"valid":true,"reason":"explains calculation order and reversed ratio","distinct_items":true,"critical_are_false":true,"explanation_teaches":true,"explanation_support_quotes":["密度等于质量除以体积，ρ=m/V。"]});
    assert!(explanation::validate_review(&checked, &q, evidence).is_ok());
    let mut invented = checked;
    invented["explanation_support_quotes"] = json!(["不存在的依据"]);
    assert!(explanation::validate_review(&invented, &q, evidence).is_err());
    let b = learnloom_m0::budget::Budget::new(4096).unwrap();
    for prompt in [
        explanation::REWRITE_PROMPT,
        explanation::CHECK_PROMPT,
        explanation::ALIGNMENT_PROMPT,
    ] {
        b.check(prompt, "{}", "", "").unwrap();
    }
}

#[tokio::test]
async fn five_question_partial_generation_failure_and_no_invalid_publication() {
    let t = tempfile::tempdir().unwrap();
    let (url, mode, _, task) = mock().await;
    let app = setup_n(t.path(), &url, 7);
    mode.store(4, Ordering::SeqCst);
    let s = app.daily_with_clock(&Fixed(10)).unwrap();
    assert_eq!(wait(&app, &s).await["status"], "succeeded");
    let s = app.review_session(s["id"].as_str().unwrap()).unwrap();
    assert_eq!(s["actual_count"], 4);
    assert!(!s["reasons"].as_array().unwrap().is_empty());
    let ids = s["questions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|q| q["point_id"].as_str().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(ids.len(), 4);
    assert!(s["questions"]
        .as_array()
        .unwrap()
        .iter()
        .all(|q| q.get("reference").is_none()));
    mode.store(5, Ordering::SeqCst);
    let next = app.daily_with_clock(&Fixed(11)).unwrap();
    wait(&app, &next).await;
    let next = app.review_session(next["id"].as_str().unwrap()).unwrap();
    assert!(next["actual_count"].as_u64().unwrap() <= 4);
    task.abort();
}

#[tokio::test]
async fn one_and_two_points_ten_days_reuse_without_inheriting_scores() {
    for n in [1, 2] {
        let t = tempfile::tempdir().unwrap();
        let (url, _, _, task) = mock().await;
        let app = setup_n(t.path(), &url, n);
        for day in 1..=10 {
            let s = app.daily_with_clock(&Fixed(day)).unwrap();
            assert_eq!(wait(&app, &s).await["status"], "succeeded");
            let s = app.review_session(s["id"].as_str().unwrap()).unwrap();
            assert_eq!(s["actual_count"], n);
            assert!(s["questions"]
                .as_array()
                .unwrap()
                .iter()
                .all(|q| q.get("attempt").is_none()));
            if day > 1 {
                assert!(s["questions"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|q| q["reused_from"].is_string()));
            }
        }
        task.abort();
    }
}

#[tokio::test]
async fn legacy_explanation_is_not_reused_in_new_daily_questions() {
    let t = tempfile::tempdir().unwrap();
    let (url, _, _, task) = mock().await;
    let app = setup(t.path(), &url);
    let first = app.daily_with_clock(&Fixed(1)).unwrap();
    wait(&app, &first).await;
    let root = std::path::PathBuf::from(
        app.bootstrap().unwrap()["library"]["path"]
            .as_str()
            .unwrap(),
    );
    let db = rusqlite::Connection::open(root.join(".learnloom/learnloom.sqlite3")).unwrap();
    db.execute("UPDATE review_questions SET rubric_details=json_remove(rubric_details,'$.quality_version')",[]).unwrap();
    let next = app.daily_with_clock(&Fixed(2)).unwrap();
    assert_eq!(wait(&app, &next).await["status"], "succeeded");
    let next = app.review_session(next["id"].as_str().unwrap()).unwrap();
    assert_eq!(next["actual_count"], 1);
    assert!(next["questions"][0]["reused_from"].is_null());
    let (reference, explanation): (String, String) = db
        .query_row(
            "SELECT reference,explanation FROM questions WHERE id=?",
            [next["questions"][0]["id"].as_str().unwrap()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_ne!(reference, explanation);
    task.abort();
}

#[test]
fn queued_review_and_grading_restore_as_interrupted_without_model_calls() {
    use learnloom_m0::learning;
    let t = tempfile::tempdir().unwrap();
    let mut lib = Library::open(&t.path().join("kb"), "recovery", true).unwrap();
    fixture::seed(&mut lib).unwrap();
    let (s, e) = learning::create(&mut lib, &Fixed(1), &Snapshot::default()).unwrap();
    let e = e.unwrap();
    assert_eq!(
        lib.db
            .conn
            .query_row("SELECT status FROM jobs WHERE id=?", [&e.job], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "queued"
    );
    let (second, execution) = learning::create(&mut lib, &Fixed(2), &Snapshot::default()).unwrap();
    let execution = execution.unwrap();
    lib.db.claim(&execution).unwrap();
    let call = lib.db.call(&execution).unwrap();
    let points = learning::points(&lib, &Fixed(2).day().unwrap()).unwrap();
    let candidates = points
        .iter()
        .take(2)
        .enumerate()
        .map(|(n, p)| learnloom_m0::store::Question {
            point_id: p.point_id.clone(),
            prompt: format!("recovery question {n}"),
            reference: p.evidence.clone(),
            explanation: p.evidence.clone(),
            required: vec!["a".into(), "b".into()],
            critical_errors: vec![],
        })
        .collect::<Vec<_>>();
    let questions = lib
        .db
        .publish_questions(&execution, &call, &second, &candidates)
        .unwrap();
    let (queued, _) = lib
        .db
        .submit(
            &questions[0],
            "queued-answer",
            "original queued answer",
            "00000000000000000001",
            &Snapshot::default(),
        )
        .unwrap();
    let (running, run) = lib
        .db
        .submit(
            &questions[1],
            "running-answer",
            "original running answer",
            "00000000000000000002",
            &Snapshot::default(),
        )
        .unwrap();
    lib.db.claim(&run).unwrap();
    let root = lib.root.clone();
    drop(lib);
    let mut lib = Library::open(&root, "", false).unwrap();
    lib.recover().unwrap();
    assert_eq!(
        learning::session(&lib, &s).unwrap()["status"],
        "interrupted"
    );
    for (aid, answer, at) in [
        (queued, "original queued answer", "00000000000000000001"),
        (running, "original running answer", "00000000000000000002"),
    ] {
        let restored = lib
            .db
            .conn
            .query_row(
                "SELECT status,answer,submitted_at,score FROM attempts WHERE id=?",
                [aid],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, Option<u8>>(3)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            restored,
            ("interrupted".into(), answer.into(), at.into(), None)
        );
    }
    assert_eq!(
        lib.db
            .conn
            .query_row("SELECT count(*) FROM model_calls", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn knowledge_update_keeps_old_question_evidence_and_formal_answer_immutable() {
    use learnloom_m0::{
        id,
        knowledge::{PreparedPage, PreparedPoint},
        learning,
    };
    let t = tempfile::tempdir().unwrap();
    let (url, _, _, task) = mock().await;
    let app = setup(t.path(), &url);
    let s = app.daily_with_clock(&Fixed(12)).unwrap();
    wait(&app, &s).await;
    let sid = s["id"].as_str().unwrap().to_string();
    let q = app.review_session(&sid).unwrap()["questions"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let root = std::path::PathBuf::from(
        app.bootstrap().unwrap()["library"]["path"]
            .as_str()
            .unwrap(),
    );
    tokio::time::sleep(Duration::from_millis(20)).await;
    drop(app);
    {
        let mut lib = Library::open(&root, "", false).unwrap();
        let raw: String = lib
            .db
            .conn
            .query_row("SELECT snapshot FROM questions WHERE id=?", [&q], |r| {
                r.get(0)
            })
            .unwrap();
        let old: learning::Point = serde_json::from_str(&raw).unwrap();
        let body = "更新后的学习资料。NEW_VERSION";
        let src = lib.save_source("update", body, "text", "update").unwrap()["source_id"]
            .as_str()
            .unwrap()
            .to_string();
        let e = lib
            .db
            .enqueue("update", "organize", &src, body, &Snapshot::default())
            .unwrap();
        lib.db.claim(&e).unwrap();
        let c = lib.db.call(&e).unwrap();
        lib.db
            .conn
            .execute(
                "INSERT INTO run_page_bases VALUES(?,?,?)",
                rusqlite::params![e.run, old.page_id, old.revision_id],
            )
            .unwrap();
        lib.publish(
            &e,
            &c,
            &src,
            &[PreparedPage {
                id: old.page_id.clone(),
                title: "更新".into(),
                scope: "M2固定资料".into(),
                definition: "更新".into(),
                body: body.into(),
                base: Some(old.revision_id.clone()),
                points: vec![PreparedPoint {
                    id: old.point_id.clone(),
                    name: old.name.clone(),
                    definition: "更新".into(),
                    evidence: body.into(),
                }],
                links: vec![],
            }],
            false,
        )
        .unwrap();
        assert_ne!(
            lib.page(&old.page_id, None).unwrap()["revision"],
            old.revision_id
        );
        assert_eq!(
            lib.db
                .conn
                .query_row("SELECT snapshot FROM questions WHERE id=?", [&q], |r| {
                    r.get::<_, String>(0)
                })
                .unwrap(),
            raw
        );
        assert!(lib
            .db
            .conn
            .execute("UPDATE questions SET reference='changed' WHERE id=?", [&q])
            .is_err());
        let _ = id();
    }
    let app = AppCore::for_test(t.path()).unwrap();
    let a = app
        .submit_review(&json!({"question_id":q,"answer":"yes","submission_key":"old-version"}))
        .unwrap();
    assert_eq!(wait(&app, &a).await["status"], "succeeded");
    let s = app.review_session(&sid).unwrap();
    assert_eq!(s["questions"][0]["attempt"]["score"], 2);
    assert!(!s["questions"][0]["context"]["evidence"]
        .as_str()
        .unwrap()
        .contains("NEW_VERSION"));
    let db = rusqlite::Connection::open(root.join(".learnloom/learnloom.sqlite3")).unwrap();
    assert!(db
        .execute("UPDATE attempts SET answer='changed'", [])
        .is_err());
    assert!(db
        .execute("UPDATE attempts SET submitted_at='future'", [])
        .is_err());
    task.abort();
}
