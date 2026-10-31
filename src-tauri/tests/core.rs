use learnloom_m0::{
    budget::{split, Budget},
    config::{Settings, Snapshot},
    id,
    store::{Execution, Grade, Question, Store, Wiki},
};
use serde_json::json;
use std::process::Command;

fn wiki() -> Wiki {
    Wiki {
        page_id: "page".into(),
        point_id: "point".into(),
        title: "催化剂".into(),
        scope: "chemistry".into(),
        body: "# 催化剂\n降低活化能，不改变平衡常数。".into(),
        evidence: "降低活化能".into(),
    }
}
fn question() -> Question {
    Question {
        point_id: "point".into(),
        prompt: "催化剂如何影响活化能和平衡？".into(),
        reference: "降低活化能，不改变平衡。".into(),
        explanation: "改变路径，不改变平衡常数。".into(),
        required: vec!["降低活化能".into(), "平衡不变".into()],
        critical_errors: vec!["改变平衡".into(), "提高温度".into()],
    }
}
fn grade(score: u8) -> Grade {
    Grade {
        score,
        matched: if score == 0 {
            vec![]
        } else if score == 1 {
            vec![0]
        } else {
            vec![0, 1]
        },
        missing: if score == 0 {
            vec![0, 1]
        } else if score == 1 {
            vec![1]
        } else {
            vec![]
        },
        critical: vec![],
        noncritical_errors: vec![],
        explanation: "M0 fixture feedback".into(),
        standard: "grading_standard_v1".into(),
    }
}
fn fixture() -> (tempfile::TempDir, Store) {
    let temp = tempfile::tempdir().unwrap();
    let mut db = Store::open(temp.path()).unwrap();
    let w = wiki();
    let s = db.save_source("固定资料", &w.body).unwrap();
    let e = db
        .enqueue("ingest", "ingest", "page", "fixture", &Snapshot::default())
        .unwrap();
    db.claim(&e).unwrap();
    let call = db.call(&e).unwrap();
    assert!(db.publish_wiki(&e, &call, &s, &w, false).unwrap());
    (temp, db)
}
fn new_question(db: &mut Store, date: &str) -> String {
    let s = db.session(date).unwrap();
    let e = db
        .enqueue(
            &format!("daily:{s}"),
            "daily",
            &s,
            date,
            &Snapshot::default(),
        )
        .unwrap();
    db.bind_session(&s, &e).unwrap();
    db.claim(&e).unwrap();
    let c = db.call(&e).unwrap();
    db.publish_questions(&e, &c, &s, &[question()]).unwrap()[0].clone()
}
fn submit(db: &mut Store, question: &str, key: &str, at: &str) -> (String, Execution, String) {
    let (aid, e) = db
        .submit(question, key, "fixture answer", at, &Snapshot::default())
        .unwrap();
    db.claim(&e).unwrap();
    let c = db.call(&e).unwrap();
    (aid, e, c)
}
#[test]
fn unicode_space_markdown_config_and_process_restart() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("中文 空格");
    let first = learnloom_m0::storage_probe(&root).unwrap();
    let second = learnloom_m0::storage_probe(&root).unwrap();
    assert_eq!(first["source_id"], second["source_id"]);
    assert_eq!(second["persisted_invocations"], 2);
    assert_eq!(second["markdown_exists"], true);
    let output = Command::new(env!("CARGO_BIN_EXE_m0-probe"))
        .arg(&root)
        .output()
        .unwrap();
    assert!(output.status.success());
    let third: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(third["persisted_invocations"], 3);
}
#[test]
fn migration_is_repeatable_and_future_version_rejected() {
    let (temp, db) = fixture();
    assert_eq!(db.version().unwrap(), 2);
    drop(db);
    let db = Store::open(temp.path()).unwrap();
    assert_eq!(db.version().unwrap(), 2);
    db.conn.execute_batch("PRAGMA user_version=99").unwrap();
    drop(db);
    assert!(Store::open(temp.path()).is_err());
}
#[test]
fn cancelled_old_run_cannot_publish_in_both_arrival_orders() {
    for old_first in [true, false] {
        let (_temp, mut db) = fixture();
        let q = new_question(&mut db, "2026-10-01");
        let (aid, old, c1) = submit(&mut db, &q, "a", "2026-10-01T00:00:00Z");
        assert!(db.stop(&old, "cancelled").unwrap());
        assert!(!db
            .grade(&old, &c1, &aid, &grade(0), "2026-10-01T00:01:00Z")
            .unwrap());
        let new = db.retry(&old, "retry-1", &Snapshot::default()).unwrap();
        assert_ne!(old.run, new.run);
        assert_eq!(
            db.retry(&old, "retry-1", &Snapshot::default()).unwrap().run,
            new.run
        );
        db.claim(&new).unwrap();
        let c2 = db.call(&new).unwrap();
        if old_first {
            assert!(!db.grade(&old, &c1, &aid, &grade(0), "later").unwrap());
        }
        assert!(db
            .grade(&new, &c2, &aid, &grade(2), "2026-10-02T00:00:00Z")
            .unwrap());
        if !old_first {
            assert!(!db.grade(&old, &c1, &aid, &grade(0), "later").unwrap());
        }
        assert!(!db.stop(&old, "failed").unwrap());
        assert!(!db.stop(&new, "cancelled").unwrap());
        let result: (i64, f64) = db
            .conn
            .query_row(
                "SELECT count,recent FROM mastery WHERE point_id='point'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(result, (1, 1.0));
    }
}
#[test]
fn call_replacement_and_old_cancel_are_fenced() {
    let (_temp, mut db) = fixture();
    let q = new_question(&mut db, "2026-10-01");
    let (aid, e, c1) = submit(&mut db, &q, "a", "2026-10-01");
    db.revoke_call(&e, &c1).unwrap();
    let c2 = db.call(&e).unwrap();
    assert!(!db.grade(&e, &c1, &aid, &grade(0), "later").unwrap());
    assert!(db.grade(&e, &c2, &aid, &grade(2), "later").unwrap());
    assert!(!db.grade(&e, &c1, &aid, &grade(0), "later").unwrap());
    assert!(db.retry(&e, "bad-retry", &Snapshot::default()).is_err());
}
#[test]
fn payload_idempotency_and_cross_connection_daily_uniqueness() {
    let (temp, mut db) = fixture();
    let q = new_question(&mut db, "2026-10-01");
    let (a, e) = db
        .submit(&q, "same", "x", "2026-10-01", &Snapshot::default())
        .unwrap();
    assert_eq!(
        db.submit(&q, "same", "x", "2026-10-02", &Snapshot::default())
            .unwrap()
            .0,
        a
    );
    assert_eq!(
        db.submit(&q, "new-key", "x", "2026-10-02", &Snapshot::default())
            .unwrap()
            .1
            .run,
        e.run
    );
    assert!(db
        .submit(&q, "same", "changed", "2026-10-02", &Snapshot::default())
        .is_err());
    let path = temp.path().to_path_buf();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
    let mut threads = vec![];
    for _ in 0..2 {
        let root = path.clone();
        let barrier = barrier.clone();
        threads.push(std::thread::spawn(move || {
            let mut db = Store::open(&root).unwrap();
            barrier.wait();
            db.session("2026-10-02").unwrap()
        }));
    }
    barrier.wait();
    let ids: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(ids[0], ids[1]);
}
#[test]
fn late_grading_uses_submitted_time_not_completion_time() {
    let (_temp, mut db) = fixture();
    let q = new_question(&mut db, "2026-10-01");
    let (aid, old, _) = submit(&mut db, &q, "a", "2026-10-01T00:00:00Z");
    db.stop(&old, "failed").unwrap();
    for day in 2..=6 {
        let date = format!("2026-10-{day:02}");
        let q = new_question(&mut db, &date);
        let (a, e, c) = submit(&mut db, &q, &date, &format!("{date}T00:00:00Z"));
        assert!(db
            .grade(&e, &c, &a, &grade(2), &format!("{date}T01:00:00Z"))
            .unwrap());
    }
    let new = db.retry(&old, "late", &Snapshot::default()).unwrap();
    db.claim(&new).unwrap();
    let c = db.call(&new).unwrap();
    db.grade(&new, &c, &aid, &grade(0), "2026-10-08T00:00:00Z")
        .unwrap();
    let recent: f64 = db
        .conn
        .query_row(
            "SELECT recent FROM mastery WHERE point_id='point'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(recent, 1.0);
    let at: String = db
        .conn
        .query_row("SELECT submitted_at FROM attempts WHERE id=?", [aid], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(at, "2026-10-01T00:00:00Z");
}
#[test]
fn sessions_and_attempts_recover_without_auto_grading() {
    let (temp, mut db) = fixture();
    let q = new_question(&mut db, "2026-10-01");
    let (_a, e, _) = submit(&mut db, &q, "a", "2026-10-01");
    drop(db);
    let mut db = Store::open(temp.path()).unwrap();
    assert_eq!(db.recover().unwrap(), 1);
    let state: String = db
        .conn
        .query_row("SELECT status FROM jobs WHERE id=?", [&e.job], |r| r.get(0))
        .unwrap();
    assert_eq!(state, "interrupted");
    let count: i64 = db
        .conn
        .query_row(
            "SELECT count(*) FROM sessions WHERE date<'2026-10-02' AND status<>'completed'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}
#[test]
fn sqlite_real_process_crash_rolls_back_pointer_and_fts() {
    let (temp, db) = fixture();
    drop(db);
    let status = Command::new(env!("CARGO_BIN_EXE_m0-probe"))
        .arg("--crash-publish")
        .arg(temp.path())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(73));
    let mut db = Store::open(temp.path()).unwrap();
    assert!(db.search("催化剂").unwrap().contains(&"page".into()));
    let title: String = db
        .conn
        .query_row("SELECT title FROM pages WHERE id='page'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(title, "催化剂");
    assert_eq!(db.recover().unwrap(), 1);
}
#[test]
fn index_pending_is_hidden_and_rebuilt_after_reopen() {
    let (temp, mut db) = fixture();
    db.mark_index_pending("page").unwrap();
    assert!(db.search("催化剂").unwrap().is_empty());
    drop(db);
    let mut db = Store::open(temp.path()).unwrap();
    db.recover().unwrap();
    assert_eq!(db.search("催化剂").unwrap(), vec!["page"]);
    db.mark_index_pending("page").unwrap();
    let mut w = wiki();
    w.body.push_str(" 新段落。");
    let sid = db.save_source("updated", &w.body).unwrap();
    let e = db
        .enqueue("updated", "ingest", "page", "new", &Snapshot::default())
        .unwrap();
    db.claim(&e).unwrap();
    let c = db.call(&e).unwrap();
    db.publish_wiki(&e, &c, &sid, &w, false).unwrap();
    db.repair_indexes().unwrap();
    let state: String = db
        .conn
        .query_row("SELECT status FROM index_tasks", [], |r| r.get(0))
        .unwrap();
    assert_eq!(state, "superseded");
}
#[test]
fn snapshots_are_immutable_and_check_results_fenced() {
    let (_temp, mut db) = fixture();
    let mut settings = Settings::default();
    let q1 = settings.begin_check();
    let q2 = settings.begin_check();
    assert!(!settings.finish_check(q1.0, &q1.1, true));
    assert!(settings.finish_check(q2.0, &q2.1, true));
    let e = db
        .enqueue("snapshot", "chat", "msg", "payload", &settings.current)
        .unwrap();
    settings.replace("http://127.0.0.1:2", "model-B", Some("m0-key-B".into()));
    settings.language("en");
    assert!(!settings.finish_check(q2.0, &q2.1, true));
    assert_eq!(settings.check_state, "untested");
    let snap = db.snapshot(&e.run).unwrap();
    assert_eq!(snap.model, "deepseek-flash");
    assert_eq!(snap.language, "zh-CN");
    db.claim(&e).unwrap();
    db.stop(&e, "cancelled").unwrap();
    let new = db.retry(&e, "new-config", &settings.current).unwrap();
    assert_eq!(db.snapshot(&new.run).unwrap(), settings.current);
}
#[test]
fn config_restart_and_credential_cross_process_roundtrip() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("settings.json");
    let mut s = Settings::default();
    s.save(&path).unwrap();
    s.language("en");
    s.save(&path).unwrap();
    assert_eq!(Settings::load(&path).unwrap().current.language, "en");
    let reference = format!("m0-{}", id());
    assert!(Command::new(env!("CARGO_BIN_EXE_m0-probe"))
        .args(["--credential-write", &reference])
        .status()
        .unwrap()
        .success());
    assert!(Command::new(env!("CARGO_BIN_EXE_m0-probe"))
        .args(["--credential-read-delete", &reference])
        .status()
        .unwrap()
        .success());
    assert!(learnloom_m0::config::read_credential(&reference).is_err());
}
#[test]
fn critical_error_overrides_partial_credit_and_invalid_grades_rejected() {
    let q = question();
    for score in 0..=2 {
        grade(score).validate(&q).unwrap();
    }
    let mut g = grade(1);
    g.critical = vec![0];
    assert!(g.validate(&q).is_err());
    g.score = 0;
    g.validate(&q).unwrap();
    g.score = 2;
    assert!(g.validate(&q).is_err());
    let mut bad = question();
    bad.reference.clear();
    assert!(bad.validate().is_err());
}

#[test]
fn provider_identifier_normalization_never_changes_scores_or_invalid_ids() {
    let mut value = serde_json::to_value(grade(1)).unwrap();
    value["matched"] = json!(["0"]);
    value["missing"] = json!(["1"]);
    Grade::from_provider(&value.to_string())
        .unwrap()
        .validate(&question())
        .unwrap();
    value["score"] = json!("1");
    assert!(Grade::from_provider(&value.to_string()).is_err());
    value["score"] = json!(1);
    value["matched"] = json!(["00"]);
    assert!(Grade::from_provider(&value.to_string()).is_err());
    value["matched"] = json!([99]);
    assert!(Grade::from_provider(&value.to_string())
        .unwrap()
        .validate(&question())
        .is_err());
}
#[test]
fn small_library_ten_days_reuses_valid_questions_and_deduplicates() {
    let (_temp, mut db) = fixture();
    for day in 1..=10 {
        let s = db.session(&format!("2026-10-{day:02}")).unwrap();
        let e = db
            .enqueue(
                &format!("daily:{s}"),
                "daily",
                &s,
                "payload",
                &Snapshot::default(),
            )
            .unwrap();
        db.bind_session(&s, &e).unwrap();
        db.claim(&e).unwrap();
        let c = db.call(&e).unwrap();
        let ids = db
            .publish_questions(&e, &c, &s, &[question(), question()])
            .unwrap();
        assert_eq!(ids.len(), 1);
    }
    let reused: i64 = db
        .conn
        .query_row(
            "SELECT count(*) FROM questions WHERE reused_from IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(reused, 9);
    let attempts: i64 = db
        .conn
        .query_row("SELECT count(*) FROM attempts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(attempts, 0);
}
#[test]
fn budget_utf8_coverage_and_limits() {
    let b = Budget::new(4096).unwrap();
    assert_eq!(
        (b.margin, b.output, b.input, b.evidence),
        (410, 1024, 2662, 933)
    );
    assert!(Budget::new(1024).is_err());
    let text = "中文长资料abc。".repeat(2000);
    assert!(text.len() > 40960);
    let chunks = split(&text, b.evidence, 128).unwrap();
    assert_eq!(
        chunks.iter().map(|c| c.text.clone()).collect::<String>(),
        text
    );
    assert!(chunks.windows(2).all(|w| w[0].end == w[1].start));
    assert!(split(&text, 64, 2)
        .unwrap_err()
        .contains("SOURCE_TOO_LARGE"));
    assert!(b.check("system", &"answer".repeat(1000), "", "").is_err());
}
#[test]
fn mastery_changes_next_point_selection() {
    let (_temp, mut db) = fixture();
    let mut other = wiki();
    other.page_id = "otherpage".into();
    other.point_id = "aaa-other".into();
    let sid = db.save_source("other", &other.body).unwrap();
    let e = db
        .enqueue("other", "ingest", "otherpage", "x", &Snapshot::default())
        .unwrap();
    db.claim(&e).unwrap();
    let c = db.call(&e).unwrap();
    db.publish_wiki(&e, &c, &sid, &other, false).unwrap();
    assert_eq!(db.selection().unwrap()[0], "aaa-other");
    let q = new_question(&mut db, "2026-10-01");
    let (a, e, c) = submit(&mut db, &q, "weak", "2026-10-01");
    db.grade(&e, &c, &a, &grade(0), "later").unwrap();
    assert_eq!(db.selection().unwrap()[0], "point");
}
#[test]
fn persisted_message_survives_cancel_and_retry_resets_sequence() {
    let (temp, mut db) = fixture();
    let e = db
        .enqueue("chat", "chat", "message", "hello", &Snapshot::default())
        .unwrap();
    db.claim(&e).unwrap();
    let c = db.call(&e).unwrap();
    db.append_message(&e, &c, "message", "中文").unwrap();
    drop(db);
    let mut db = Store::open(temp.path()).unwrap();
    assert_eq!(db.message("message").unwrap()["body"], "中文");
    db.recover().unwrap();
    let before = db.message("message").unwrap();
    assert_eq!(before["status"], "interrupted");
    let new = db.retry(&e, "retrychat", &Snapshot::default()).unwrap();
    db.claim(&new).unwrap();
    let nc = db.call(&new).unwrap();
    assert!(!db.append_message(&e, &c, "message", "OLD").unwrap());
    db.append_message(&new, &nc, "message", "新正文").unwrap();
    let after = db.message("message").unwrap();
    assert_eq!(after["body"], "新正文");
    assert!(after["sequence"].as_i64() > before["sequence"].as_i64());
    assert_eq!(json!(after["references"]), json!([]));
}

#[test]
fn overlapping_sources_keep_point_identity_and_old_question_snapshot() {
    let (_temp, mut db) = fixture();
    let q = new_question(&mut db, "2026-10-01");
    let old_snapshot: String = db
        .conn
        .query_row("SELECT snapshot FROM questions WHERE id=?", [&q], |r| {
            r.get(0)
        })
        .unwrap();
    let mut updated = wiki();
    updated.body.push_str(" 催化剂反应前后质量不变。");
    updated.evidence = "催化剂反应前后质量不变".into();
    let sid = db
        .save_source("追加资料", "催化剂反应前后质量不变。")
        .unwrap();
    let e = db
        .enqueue("update", "ingest", "page", "update", &Snapshot::default())
        .unwrap();
    db.claim(&e).unwrap();
    let c = db.call(&e).unwrap();
    db.publish_wiki(&e, &c, &sid, &updated, false).unwrap();
    let mut other = wiki();
    other.page_id = "organization-page".into();
    other.point_id = "organization-point".into();
    other.scope = "organization".into();
    other.body = "催化剂在这里是推动组织变革的比喻，不代表化学物质。".into();
    other.evidence = "推动组织变革的比喻".into();
    let sid = db.save_source("同名不同概念", &other.body).unwrap();
    let e = db
        .enqueue("org", "ingest", &other.page_id, "org", &Snapshot::default())
        .unwrap();
    db.claim(&e).unwrap();
    let c = db.call(&e).unwrap();
    db.publish_wiki(&e, &c, &sid, &other, false).unwrap();
    let count: i64 = db
        .conn
        .query_row(
            "SELECT count(*) FROM points WHERE name='催化剂'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 2);
    let unchanged: String = db
        .conn
        .query_row("SELECT snapshot FROM questions WHERE id=?", [&q], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(old_snapshot, unchanged);
    let (a, e, c) = submit(&mut db, &q, "old-question", "2026-10-03");
    assert!(db.grade(&e, &c, &a, &grade(2), "2026-10-03").unwrap());
}
#[test]
fn stale_page_model_result_cannot_overwrite_newer_revision() {
    let (_temp, mut db) = fixture();
    let sid = db.save_source("source", "降低活化能 新旧内容。").unwrap();
    let old = db
        .enqueue("old-merge", "ingest", "page", "old", &Snapshot::default())
        .unwrap();
    db.claim(&old).unwrap();
    let old_call = db.call(&old).unwrap();
    let newer = db
        .enqueue("new-merge", "ingest", "page", "new", &Snapshot::default())
        .unwrap();
    db.claim(&newer).unwrap();
    let new_call = db.call(&newer).unwrap();
    assert!(db
        .publish_wiki(&newer, &new_call, &sid, &wiki(), false)
        .unwrap());
    assert!(db
        .publish_wiki(&old, &old_call, &sid, &wiki(), false)
        .unwrap_err()
        .contains("page base revision"));
}
#[test]
fn schema_one_is_upgraded_to_two_without_losing_existing_content() {
    let temp = tempfile::tempdir().unwrap();
    let conn = rusqlite::Connection::open(temp.path().join("m0.sqlite3")).unwrap();
    conn.execute_batch(include_str!("../migrations/001_m0.sql"))
        .unwrap();
    conn.execute("INSERT INTO probe_counter VALUES(1,7)", [])
        .unwrap();
    drop(conn);
    let db = Store::open(temp.path()).unwrap();
    assert_eq!(db.version().unwrap(), 2);
    let n: i64 = db
        .conn
        .query_row("SELECT n FROM probe_counter", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 7);
}
#[test]
fn invalid_question_foreign_key_rolls_back_attempt_and_job_together() {
    let (_temp, mut db) = fixture();
    let before: i64 = db
        .conn
        .query_row("SELECT count(*) FROM jobs", [], |r| r.get(0))
        .unwrap();
    assert!(db
        .submit(
            "missing-question",
            "bad",
            "answer",
            "2026-10-01",
            &Snapshot::default()
        )
        .is_err());
    let after: i64 = db
        .conn
        .query_row("SELECT count(*) FROM jobs", [], |r| r.get(0))
        .unwrap();
    assert_eq!(before, after);
}

#[test]
fn two_writers_submit_and_retry_the_same_operation_once() {
    let (temp, mut db) = fixture();
    let question = new_question(&mut db, "2026-10-01");
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
    let mut threads = vec![];
    for _ in 0..2 {
        let root = temp.path().to_path_buf();
        let q = question.clone();
        let barrier = barrier.clone();
        threads.push(std::thread::spawn(move || {
            let mut db = Store::open(&root).unwrap();
            barrier.wait();
            db.submit(
                &q,
                "concurrent-submit",
                "answer",
                "2026-10-01",
                &Snapshot::default(),
            )
            .unwrap()
        }));
    }
    barrier.wait();
    let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(results[0].0, results[1].0);
    assert_eq!(results[0].1.run, results[1].1.run);
    let e = results[0].1.clone();
    db.stop(&e, "cancelled").unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
    let mut threads = vec![];
    for _ in 0..2 {
        let root = temp.path().to_path_buf();
        let old = e.clone();
        let barrier = barrier.clone();
        threads.push(std::thread::spawn(move || {
            let mut db = Store::open(&root).unwrap();
            barrier.wait();
            db.retry(&old, "concurrent-retry", &Snapshot::default())
                .unwrap()
        }));
    }
    barrier.wait();
    let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(results[0].run, results[1].run);
    assert!(!db.stop(&e, "cancelled").unwrap());
}
