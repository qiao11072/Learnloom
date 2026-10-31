//! Daily review: immutable evidence, explicit paid work, and durable answer slots.
use crate::{
    app::AppCore,
    config::Snapshot,
    hash, id,
    knowledge::{now, Library},
    store::{enqueue_in_transaction, Execution, Question, Store},
    workflow, Result,
};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeSet, sync::Arc};
use tokio_util::sync::CancellationToken;
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[derive(Clone, Debug)]
pub struct Day {
    pub date: String,
    pub timezone: String,
    pub offset_minutes: i32,
    pub utc_ms: i64,
}
pub trait Clock {
    fn day(&self) -> Result<Day>;
}
pub struct DeviceClock;
impl Clock for DeviceClock {
    fn day(&self) -> Result<Day> {
        #[cfg(windows)]
        {
            #[repr(C)]
            #[derive(Default, Clone, Copy)]
            struct SystemTime {
                year: u16,
                month: u16,
                weekday: u16,
                day: u16,
                hour: u16,
                minute: u16,
                second: u16,
                millis: u16,
            }
            #[repr(C)]
            struct Zone {
                bias: i32,
                standard_name: [u16; 32],
                standard_date: SystemTime,
                standard_bias: i32,
                daylight_name: [u16; 32],
                daylight_date: SystemTime,
                daylight_bias: i32,
            }
            #[link(name = "kernel32")]
            extern "system" {
                fn GetLocalTime(t: *mut SystemTime);
                fn GetTimeZoneInformation(z: *mut Zone) -> u32;
            }
            let mut t = SystemTime::default();
            let mut z: Zone = unsafe { std::mem::zeroed() };
            let state = unsafe {
                GetLocalTime(&mut t);
                GetTimeZoneInformation(&mut z)
            };
            if state == u32::MAX {
                return Err("CLOCK_ERROR".into());
            }
            let offset = -(z.bias
                + if state == 2 {
                    z.daylight_bias
                } else {
                    z.standard_bias
                });
            let name = if state == 2 {
                &z.daylight_name
            } else {
                &z.standard_name
            };
            Ok(Day {
                date: format!("{:04}-{:02}-{:02}", t.year, t.month, t.day),
                timezone: String::from_utf16_lossy(
                    &name[..name.iter().position(|v| *v == 0).unwrap_or(32)],
                ),
                offset_minutes: offset,
                utc_ms: now(),
            })
        }
        #[cfg(not(windows))]
        {
            Err("CLOCK_ERROR: Windows device clock required".into())
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Point {
    #[serde(default)]
    pub point_evidence: String,
    pub point_id: String,
    pub page_id: String,
    pub revision_id: String,
    pub evidence: String,
    pub evidence_hash: String,
    pub revision_hash: String,
    pub name: String,
    pub difficulty: String,
    pub reason: String,
    pub priority: f64,
    #[serde(default)]
    pub policy_input: Value,
}
pub fn points(lib: &Library, day: &Day) -> Result<Vec<Point>> {
    let mut s=lib.db.conn.prepare("SELECT p.id,p.page_id,p.revision_id,p.evidence,r.hash,p.name,m.recent,m.weak FROM points p JOIN pages w ON w.id=p.page_id JOIN revisions r ON r.id=p.revision_id LEFT JOIN mastery m ON m.point_id=p.id WHERE p.revision_id=w.current_revision AND w.index_state='ready' ORDER BY p.id").map_err(err)?;
    let rows = s
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, Option<f64>>(6)?,
                r.get::<_, Option<bool>>(7)?,
            ))
        })
        .map_err(err)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(err)?;
    let mut groups: [Vec<Point>; 3] = Default::default();
    for (pid, page, rev, evidence, rhash, name, _recent, _weak) in rows {
        // Reading checks the immutable file hash as well as database metadata.
        let page_data = lib.page(&page, Some(&rev))?;
        if evidence.trim().is_empty()
            || !page_data["body"].as_str().unwrap_or("").contains(&evidence)
        {
            continue;
        }
        let performance = crate::mastery::summary(&lib.db.conn,&pid)?;
        let recent = performance.recent_score;
        let weak = performance.weak;
        let zeros = performance.consecutive_zero.min(2);
        let last:Option<i64>=lib.db.conn.query_row("SELECT max(rs.created_at) FROM questions q JOIN review_sessions rs ON rs.session_id=q.session_id WHERE q.point_id=?",[&pid],|r|r.get(0)).map_err(err)?;
        let days = last
            .map(|v| ((day.utc_ms - v).max(0) as f64 / 86400000.0).min(7.0))
            .unwrap_or(7.0);
        let group = if weak == Some(true) {
            0
        } else if recent.is_none() {
            1
        } else {
            2
        };
        let point_evidence = evidence.clone();
        let evidence = if page_data["body"].as_str().unwrap_or("").len() <= 900 {
            page_data["body"].as_str().unwrap().to_owned()
        } else {
            evidence
        };
        groups[group].push(Point {
            point_evidence,
            point_id: pid,
            page_id: page,
            revision_id: rev,
            evidence_hash: hash(&evidence),
            evidence,
            revision_hash: rhash,
            name,
            difficulty: if zeros >= 2 {
                "basic"
            } else if performance.consecutive_full >= 2 {
                "application"
            } else {
                "understanding"
            }
            .into(),
            reason: ["weak", "unseen", "stale"][group].into(),
            policy_input: json!({"policy_version":"policy_v1","performance":performance,"last_question_at":last,"days_since_question":days,"sort":"priority desc / point_id asc; unseen never-covered first"}),
            priority: 0.60 * (1.0 - recent.unwrap_or(0.0))
                + 0.25 * days / 7.0
                + 0.15 * zeros as f64 / 2.0,
        });
    }
    for (i, g) in groups.iter_mut().enumerate() {
        g.sort_by(|a, b| {
            if i == 1 {
                a.policy_input["last_question_at"].as_i64().is_some().cmp(&b.policy_input["last_question_at"].as_i64().is_some()).then(a.point_id.cmp(&b.point_id))
            } else {
                b.priority
                    .total_cmp(&a.priority)
                    .then(a.point_id.cmp(&b.point_id))
            }
        });
    }
    let n = groups.iter().map(Vec::len).sum::<usize>().min(5);
    let quota = match n {
        0 => [0, 0, 0],
        1 => [1, 0, 0],
        2 => [1, 1, 0],
        3 => [2, 1, 0],
        4 => [2, 1, 1],
        _ => [3, 1, 1],
    };
    let mut out = vec![];
    for i in 0..3 {
        let take = quota[i].min(groups[i].len());
        out.extend(groups[i].drain(..take));
    }
    for g in &mut groups {
        let take = (n - out.len()).min(g.len());
        out.extend(g.drain(..take));
    }
    for p in &mut out {
        p.policy_input["date"] = json!(day.date);
        p.policy_input["quota"] = json!(quota);
        p.policy_input["target_count"] = json!(n);
    }
    Ok(out)
}
pub fn list(lib: &Library) -> Result<Value> {
    let mut s=lib.db.conn.prepare("SELECT s.id,s.date,s.status,s.actual_count,s.target_count FROM sessions s ORDER BY s.date DESC,s.id").map_err(err)?;
    let items=s.query_map([],|r|Ok(json!({"id":r.get::<_,String>(0)?,"date":r.get::<_,String>(1)?,"status":r.get::<_,String>(2)?,"actual_count":r.get::<_,i64>(3)?,"target_count":r.get::<_,i64>(4)?}))).map_err(err)?.collect::<std::result::Result<Vec<_>,_>>().map_err(err)?;
    Ok(json!({"items":items}))
}
pub fn session(lib: &Library, sid: &str) -> Result<Value> {
    let tx = lib.db.conn.unchecked_transaction().map_err(err)?;
    let mut result=tx.query_row("SELECT s.date,s.status,s.actual_count,s.target_count,s.job_id,s.active_run,s.seq,COALESCE(j.error,''),rs.reasons,rs.timezone,rs.offset_minutes FROM sessions s JOIN review_sessions rs ON rs.session_id=s.id LEFT JOIN job_meta j ON j.job_id=s.job_id WHERE s.id=?",[sid],|r|Ok(json!({"id":sid,"date":r.get::<_,String>(0)?,"status":r.get::<_,String>(1)?,"actual_count":r.get::<_,i64>(2)?,"target_count":r.get::<_,i64>(3)?,"job_id":r.get::<_,Option<String>>(4)?,"run_id":r.get::<_,Option<String>>(5)?,"sequence":r.get::<_,i64>(6)?,"error":r.get::<_,String>(7)?,"reasons":serde_json::from_str::<Value>(&r.get::<_,String>(8)?).unwrap_or(json!([])),"timezone":r.get::<_,String>(9)?,"offset_minutes":r.get::<_,i64>(10)?}))).map_err(err)?;
    let mut s=tx.prepare("SELECT q.id,q.point_id,q.prompt,q.reference,q.explanation,q.rubric,q.snapshot,q.reused_from,d.answer,d.revision,a.id,a.answer,a.submitted_at,a.status,a.score,a.feedback,a.job_id,a.active_run,COALESCE(j.error,'') FROM questions q LEFT JOIN answer_drafts d ON d.question_id=q.id LEFT JOIN attempts a ON a.question_id=q.id LEFT JOIN job_meta j ON j.job_id=a.job_id WHERE q.session_id=? ORDER BY q.ordinal").map_err(err)?;
    result["questions"]=json!(s.query_map([sid],|r|{
  let aid:Option<String>=r.get(10)?;
  let mut q=json!({"id":r.get::<_,String>(0)?,"point_id":r.get::<_,String>(1)?,"prompt":r.get::<_,String>(2)?,"reused_from":r.get::<_,Option<String>>(7)?,"draft":r.get::<_,Option<String>>(8)?.unwrap_or_default(),"draft_revision":r.get::<_,Option<i64>>(9)?.unwrap_or(0)});
  if let Some(aid)=aid{q["attempt"]=json!({"id":aid,"answer":r.get::<_,String>(11)?,"submitted_at":r.get::<_,String>(12)?,"status":r.get::<_,String>(13)?,"score":r.get::<_,Option<u8>>(14)?,"feedback":r.get::<_,Option<String>>(15)?.and_then(|s|serde_json::from_str::<Value>(&s).ok()),"job_id":r.get::<_,String>(16)?,"run_id":r.get::<_,String>(17)?,"error":r.get::<_,String>(18)?});
   q["reference"]=json!(r.get::<_,String>(3)?);q["explanation"]=json!(r.get::<_,String>(4)?);q["rubric"]=serde_json::from_str(&r.get::<_,String>(5)?).unwrap_or(Value::Null);q["context"]=serde_json::from_str(&r.get::<_,String>(6)?).unwrap_or(Value::Null);
  } Ok(q)
 }).map_err(err)?.collect::<std::result::Result<Vec<_>,_>>().map_err(err)?);
    drop(s);
    tx.commit().map_err(err)?;
    Ok(result)
}
pub fn create(
    lib: &mut Library,
    clock: &dyn Clock,
    snapshot: &Snapshot,
) -> Result<(String, Option<Execution>)> {
    let day = clock.day()?;
    if let Some(s) = lib
        .db
        .conn
        .query_row("SELECT id FROM sessions WHERE date=?", [&day.date], |r| {
            r.get(0)
        })
        .optional()
        .map_err(err)?
    {
        return Ok((s, None));
    }
    let selected = points(lib, &day)?;
    if selected.is_empty() {
        return Err("NO_VALID_KNOWLEDGE: 请先保存并整理学习资料".into());
    }
    let tx = lib
        .db
        .conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(err)?;
    // The transaction and unique date also protect callers outside AppCore's writer lock.
    if let Some(s) = tx
        .query_row("SELECT id FROM sessions WHERE date=?", [&day.date], |r| {
            r.get(0)
        })
        .optional()
        .map_err(err)?
    {
        return Ok((s, None));
    }
    let sid = id();
    let e = enqueue_in_transaction(
        &tx,
        &format!("review:{}", day.date),
        "review",
        &sid,
        &day.date,
        snapshot,
    )?;
    tx.execute(
        "INSERT INTO sessions(id,date,status,job_id,active_run) VALUES(?,?,'generating',?,?)",
        params![sid, day.date, e.job, e.run],
    )
    .map_err(err)?;
    tx.execute(
        "INSERT INTO review_sessions VALUES(?,?,?,?,?, '[]')",
        params![
            sid,
            day.timezone,
            day.offset_minutes,
            day.utc_ms,
            serde_json::to_string(&selected).map_err(err)?
        ],
    )
    .map_err(err)?;
    tx.execute(
        "INSERT INTO job_meta VALUES(?,'queued','',?,?)",
        params![e.job, now(), now()],
    )
    .map_err(err)?;
    tx.commit().map_err(err)?;
    Ok((sid, Some(e)))
}
pub fn draft(lib: &mut Library, qid: &str, answer: &str, revision: i64) -> Result<Value> {
    if answer.len() > 32000 {
        return Err("INPUT_TOO_LONG".into());
    }
    let tx = lib
        .db
        .conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .map_err(err)?;
    let exists: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM attempts WHERE question_id=?)",
            [qid],
            |r| r.get(0),
        )
        .map_err(err)?;
    if exists {
        return Err("ALREADY_SUBMITTED".into());
    }
    let old: Option<(String, i64)> = tx
        .query_row(
            "SELECT answer,revision FROM answer_drafts WHERE question_id=?",
            [qid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(err)?;
    if let Some((text, v)) = &old {
        if text == answer {
            return Ok(json!({"revision":v}));
        }
        if *v != revision {
            return Err("CONFLICT: 草稿已更新".into());
        }
    } else if revision != 0 {
        return Err("CONFLICT: 草稿版本".into());
    }
    tx.execute("INSERT INTO answer_drafts VALUES(?,?,?,?) ON CONFLICT(question_id) DO UPDATE SET answer=excluded.answer,revision=excluded.revision,updated_at=excluded.updated_at",params![qid,answer,revision+1,now()]).map_err(err)?;
    tx.commit().map_err(err)?;
    Ok(json!({"revision":revision+1}))
}
impl AppCore {
    pub fn review_list(&self) -> Result<Value> {
        let i = self.inner.lock().map_err(|_| "STATE_LOCK")?;
        list(i.library.as_ref().ok_or("NO_LIBRARY")?)
    }
    pub fn review_session(&self, sid: &str) -> Result<Value> {
        let i = self.inner.lock().map_err(|_| "STATE_LOCK")?;
        session(i.library.as_ref().ok_or("NO_LIBRARY")?, sid)
    }
    pub fn daily(self: &Arc<Self>) -> Result<Value> {
        self.daily_with_clock(&DeviceClock)
    }
    pub fn daily_with_clock(self: &Arc<Self>, clock: &dyn Clock) -> Result<Value> {
        let (kb, sid, e) = {
            let mut i = self.inner.lock().map_err(|_| "STATE_LOCK")?;
            // Existing daily sessions remain readable without a working model configuration.
            let day = clock.day()?;
            let lib = i.library.as_ref().ok_or("NO_LIBRARY")?;
            if let Some(sid) = lib
                .db
                .conn
                .query_row("SELECT id FROM sessions WHERE date=?", [&day.date], |r| {
                    r.get::<_, String>(0)
                })
                .optional()
                .map_err(err)?
            {
                return session(lib, &sid);
            }
            let snapshot = Self::model_snapshot(&i)?;
            let lib = i.library.as_mut().unwrap();
            let (s, e) = create(lib, clock, &snapshot)?;
            (lib.marker.id.clone(), s, e)
        };
        if let Some(e) = e {
            self.schedule(kb, e)
        }
        self.review_session(&sid)
    }
    pub fn save_draft(&self, v: &Value) -> Result<Value> {
        let mut i = self.inner.lock().map_err(|_| "STATE_LOCK")?;
        draft(
            i.library.as_mut().ok_or("NO_LIBRARY")?,
            arg(v, "question_id")?,
            arg(v, "answer")?,
            v["revision"].as_i64().ok_or("INVALID_ARGUMENT")?,
        )
    }
    pub fn submit_review(self: &Arc<Self>, v: &Value) -> Result<Value> {
        let (kb, aid, e) = {
            let mut i = self.inner.lock().map_err(|_| "STATE_LOCK")?;
            let qid = arg(v, "question_id")?;
            let answer = arg(v, "answer")?;
            let key = arg(v, "submission_key")?;
            let lib = i.library.as_ref().ok_or("NO_LIBRARY")?;
            if let Some((aid,job,run,old))=lib.db.conn.query_row("SELECT id,job_id,active_run,request_hash FROM attempts WHERE question_id=? OR submission_key=?",params![qid,key],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?))).optional().map_err(err)? {
                if old!=hash(&format!("{qid}\n{answer}")){return Err("CONFLICT".into())}
                return Ok(json!({"attempt_id":aid,"job_id":job,"run_id":run}));
            }
            let snapshot = Self::model_snapshot(&i)?;
            let lib = i.library.as_mut().ok_or("NO_LIBRARY")?;
            let answer = arg(v, "answer")?;
            if answer.len() > 32000 {
                return Err("INPUT_TOO_LONG".into());
            }
            let (a, e) = lib.db.submit(
                arg(v, "question_id")?,
                arg(v, "submission_key")?,
                answer,
                &format!("{:020}", now()),
                &snapshot,
            )?;
            lib.db
                .conn
                .execute(
                    "INSERT OR IGNORE INTO job_meta VALUES(?,'queued','',?,?)",
                    params![e.job, now(), now()],
                )
                .map_err(err)?;
            (lib.marker.id.clone(), a, e)
        };
        // Scheduling a terminal slot only attempts claim; never starts another model call.
        self.schedule(kb, e.clone());
        Ok(json!({"attempt_id":aid,"job_id":e.job,"run_id":e.run}))
    }
}
fn arg<'a>(v: &'a Value, k: &str) -> Result<&'a str> {
    v[k].as_str()
        .ok_or_else(|| format!("INVALID_ARGUMENT: {k}"))
}

/// Reject isolated substrings that discard their surrounding claim. This is a
/// context check, not a keyword classifier; semantic assessment stays with v6.
pub fn contextual_grade(raw: &Value, q: &Question, answer: &str) -> Result<crate::store::Grade> {
    for field in ["matched", "critical"] {
        for hit in raw[field].as_array().into_iter().flatten() {
            let quote = hit["quote"].as_str().ok_or("INVALID_ASSESSMENT: quote")?;
            if quote.trim().is_empty() {
                return Err("INVALID_ASSESSMENT: empty quote".into());
            }
            let contextual = answer.match_indices(quote).any(|(start, _)| {
                let before = &answer[..start];
                let after = &answer[start + quote.len()..];
                let boundary = |c: char| "。！？.!?\n；;".contains(c);
                before
                    .chars()
                    .rev()
                    .take_while(|c| !boundary(*c))
                    .all(char::is_whitespace)
                    && (quote.chars().last().map(boundary).unwrap_or(false)
                        || after
                            .chars()
                            .take_while(|c| !boundary(*c))
                            .all(char::is_whitespace))
            });
            if !contextual {
                return Err(
                    "INVALID_ASSESSMENT: quote must retain complete sentence context".into(),
                );
            }
        }
    }
    workflow::assessment_grade(&raw.to_string(), q, answer)
}

pub async fn run(
    app: Arc<AppCore>,
    kb: String,
    e: Execution,
    cancel: CancellationToken,
) -> Result<()> {
    let ee = e.clone();
    let (kind, entity, snapshot) = app
        .blocking(kb.clone(), move |lib| {
            lib.db.claim(&ee)?;
            let (k, id) = lib
                .db
                .conn
                .query_row(
                    "SELECT kind,entity_id FROM jobs WHERE id=?",
                    [&ee.job],
                    |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
                )
                .map_err(err)?;
            Ok((k, id, lib.db.snapshot(&ee.run)?))
        })
        .await?;
    if kind == "grade" {
        return grade_run(app, kb, e, entity, snapshot, cancel).await;
    }
    generate_run(app, kb, e, entity, snapshot, cancel).await
}
async fn grade_run(
    app: Arc<AppCore>,
    kb: String,
    e: Execution,
    aid: String,
    snapshot: Snapshot,
    cancel: CancellationToken,
) -> Result<()> {
    let a = aid.clone();
    let(q,answer,context)=app.blocking(kb.clone(),move|lib|{lib.db.conn.query_row("SELECT q.rubric,a.answer,q.snapshot FROM attempts a JOIN questions q ON q.id=a.question_id WHERE a.id=?",[a],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?))).map_err(err)}).await?;
    let q: Question = serde_json::from_str(&q).map_err(err)?;
    let context: Value = serde_json::from_str(&context).map_err(err)?;
    let mut input = workflow::grade_input(
        &q,
        &answer,
        context["evidence"].as_str().ok_or("INVALID_EVIDENCE")?,
    );
    input["language"] = json!(snapshot.language);
    input["quote_context_rule"]=json!("For EVERY matched/critical hit copy the ENTIRE answer verbatim as quote, including all sentences and punctuation. Identical full-answer quotes are allowed. If genuinely ambiguous return {ungradable:true}; never guess zero.");
    for index in 0..2 {
        let response = crate::ingest::rpc(
            &app,
            &kb,
            &e,
            &snapshot,
            workflow::GRADE_PROMPT,
            &input,
            "assessment-v6",
            &cancel,
        )
        .await;
        match response {
            Ok((raw, call)) => match contextual_grade(&raw, &q, &answer) {
                Ok(g) => {
                    let ee = e.clone();
                    let aa = aid.clone();
                    return app
                        .blocking(kb, move |lib| {
                            if !lib.db.grade_recorded(
                                &ee,
                                &call,
                                &aa,
                                &g,
                                &format!("{:020}", now()),
                                Some(&raw.to_string()),
                            )? {
                                return Err("STALE_RUN".into());
                            }
                            Ok(())
                        })
                        .await;
                }
                Err(error) => {
                    if index == 1 {
                        return Err(format!("INVALID_OUTPUT: {error}"));
                    }
                    input["correction_error"] = json!(error);
                }
            },
            Err(error) => {
                if index == 1
                    || !error.starts_with("INVALID_OUTPUT")
                        && !error.starts_with("expected")
                        && !error.starts_with("EOF")
                {
                    return Err(error);
                }
                input["correction_error"] =
                    json!("Return complete assessment JSON; no score. Ambiguity must fail.");
            }
        }
    }
    Err("INVALID_OUTPUT".into())
}
async fn generate_run(
    app: Arc<AppCore>,
    kb: String,
    e: Execution,
    sid: String,
    snapshot: Snapshot,
    cancel: CancellationToken,
) -> Result<()> {
    let s = sid.clone();
    let selected: Vec<Point> = app
        .blocking(kb.clone(), move |lib| {
            let raw: String = lib
                .db
                .conn
                .query_row(
                    "SELECT selection FROM review_sessions WHERE session_id=?",
                    [s],
                    |r| r.get(0),
                )
                .map_err(err)?;
            serde_json::from_str(&raw).map_err(err)
        })
        .await?;
    let mut candidates = vec![];
    let mut reasons = vec![];
    let mut fingerprints = BTreeSet::new();
    let mut last_call = None;
    for point in selected {
        let p = point.clone();
        let s = sid.clone();
        let history:Vec<(String,Question)>=app.blocking(kb.clone(),move|lib|{
   let mut st=lib.db.conn.prepare("SELECT q.id,q.rubric,q.snapshot FROM questions q JOIN sessions s ON s.id=q.session_id JOIN review_questions rq ON rq.question_id=q.id WHERE q.point_id=? AND json_extract(rq.rubric_details,'$.quality_version')='question-v6' AND json_extract(rq.rubric_details,'$.rubric_check_version')='rubric-v2' AND json_extract(rq.rubric_details,'$.publication_guard') IN ('guard-v1','guard-v2','guard-v3') ORDER BY s.date,q.id").map_err(err)?;
   let rows=st.query_map([&p.point_id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?))).map_err(err)?.collect::<std::result::Result<Vec<_>,_>>().map_err(err)?;
   let _=s;Ok(rows.into_iter().filter_map(|(id,q,ctx)|{let old:Point=serde_json::from_str(&ctx).ok()?;if old.revision_hash!=p.revision_hash||old.evidence_hash!=p.evidence_hash||old.revision_id!=p.revision_id{return None}let q:Question=serde_json::from_str(&q).ok()?;q.validate().ok()?;crate::explanation::publication(&q).ok()?;Some((id,q))}).collect())
  }).await?;
        let p = point.point_id.clone();
        let current = sid.clone();
        let recent:Vec<Question>=app.blocking(kb.clone(),move|lib|{
            let mut st=lib.db.conn.prepare("SELECT q.rubric FROM questions q JOIN sessions s ON s.id=q.session_id WHERE q.point_id=?1 AND s.date>=date((SELECT date FROM sessions WHERE id=?2),'-7 days') AND s.date<=(SELECT date FROM sessions WHERE id=?2) ORDER BY s.date DESC,q.id LIMIT 7").map_err(err)?;
            let rows=st.query_map(params![p,current],|r|r.get::<_,String>(0)).map_err(err)?.collect::<std::result::Result<Vec<_>,_>>().map_err(err)?;
            Ok(rows.into_iter().filter_map(|raw|serde_json::from_str(&raw).ok()).collect())
        }).await?;
        let mut input = json!({"point_id":point.point_id,"focus":point.name,"evidence":point.evidence,"difficulty":point.difficulty,"language":snapshot.language,"recent_prompts":recent.iter().map(|q|q.prompt.clone()).collect::<Vec<_>>(),
            "question_rules":"Never quote the evidence or answer facts in the prompt. Ask two genuinely independent facts from the supplied context if the focus alone has one fact. A condition and its restatement are ONE item. Keep question and explanation concise."});
        let mut accepted = None;
        for index in 0..2 {
            match crate::ingest::rpc(
                &app,
                &kb,
                &e,
                &snapshot,
                workflow::QUESTION_PROMPT,
                &input,
                "question-v6",
                &cancel,
            )
            .await
            {
                Ok((raw, call)) => {
                    last_call = Some(call);
                    let parsed = serde_json::from_value::<Question>(raw)
                        .map_err(err)
                        .and_then(|q| {
                            q.validate()?;
                            if !point.evidence.contains(&q.reference) {
                                return Err(
                                    "INVALID_EVIDENCE: reference must quote evidence".into()
                                );
                            }
                            crate::explanation::publication(&q)?;
                            if q.point_id != point.point_id {
                                return Err("INVALID_POINT".into());
                            }
                            Ok(q)
                        });
                    match parsed {
                        Ok(q) => {
                            let repeated = recent
                                .iter()
                                .any(|old| old.fingerprint() == q.fingerprint());
                            if !repeated && !fingerprints.contains(&q.fingerprint()) {
                                accepted = Some((q, None));
                                break;
                            }
                            if index == 0 {
                                input["rewrite_instruction"] = json!(
                                    "Ask a different grounded angle. Do not repeat this prompt."
                                );
                                input["recent_prompt"] = json!(q.prompt);
                            } else if repeated {
                                accepted = history
                                    .iter()
                                    .find(|(_, old)| old.fingerprint() == q.fingerprint())
                                    .map(|(id, old)| (old.clone(), Some(id.clone())));
                                if accepted.is_none() {
                                    // A legacy stem can get a newly checked explanation without copying an invalid old explanation.
                                    reasons.push(json!({"point_id":point.point_id,"reason":"same_stem_new_explanation_requires_validation"}));
                                    accepted = Some((q, None));
                                }
                            }
                        }
                        Err(error) => {
                            input["schema_correction"] = json!(error);
                        }
                    }
                }
                Err(error) => {
                    if cancel.is_cancelled()
                        || error.starts_with("CANCELLED")
                        || error.starts_with("STALE")
                        || error.starts_with("TOKEN_BUDGET")
                    {
                        return Err(error);
                    }
                    reasons.push(json!({"point_id":point.point_id,"error":error}));
                    break;
                }
            }
        }
        // One independent alignment check per candidate, inside the 3 x target RPC ceiling.
        if let Some((candidate, None)) = &accepted {
            let check = json!({"candidate":candidate,"evidence":point.evidence,
                "quote_source_rule":"All support quotes MUST be verbatim evidence, NEVER explanation text. Cite source facts even for logical deductions.",
                "rubric_review_rules":"Each independent item needs a DIFFERENT minimal evidence quote. Equivalent conditions or formula/words are ONE item. Correct paraphrases are never critical errors. Reject prompts containing the answer."});
            let system = crate::explanation::ALIGNMENT_PROMPT;
            match crate::ingest::rpc(
                &app,
                &kb,
                &e,
                &snapshot,
                system,
                &check,
                "question-alignment-v2",
                &cancel,
            )
            .await
            {
                Ok((raw, call)) => {
                    last_call = Some(call);
                    if crate::explanation::validate_alignment(&raw, candidate, &point.evidence)
                        .is_err()
                    {
                        reasons.push(json!({"point_id":point.point_id,"alignment_rejection":raw}));
                        accepted = None;
                    }
                }
                Err(error) => {
                    if cancel.is_cancelled() || error.starts_with("CANCELLED") {
                        return Err(error);
                    }
                    reasons.push(json!({"point_id":point.point_id,"alignment_failure":error}));
                    accepted = None;
                }
            }
        }
        if accepted.is_none() {
            accepted = history
                .iter()
                .find(|(_, q)| !fingerprints.contains(&q.fingerprint()))
                .map(|(id, q)| (q.clone(), Some(id.clone())));
        }
        if let Some((q, reused)) = accepted {
            if fingerprints.insert(q.fingerprint()) {
                candidates.push((point, q, reused));
            }
        } else {
            reasons.push(json!({"point_id":point.point_id,"reason":"no_valid_candidate / no_reusable_question"}));
        }
    }
    if candidates.is_empty() {
        return Err(format!(
            "NO_VALID_QUESTION: 没有完整且有依据的题目: {}",
            json!(reasons)
        ));
    }
    let ee = e.clone();
    app.blocking(kb,move|lib|{
  let call:String=lib.db.conn.query_row("SELECT active_call FROM runs WHERE id=?",[&ee.run],|r|r.get(0)).map_err(err)?;let _=last_call;let tx=lib.db.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(err)?;
  if !Store::owns(&tx,&ee,&call)?{return Err("STALE_RUN".into())}
  let n=candidates.len();if n<5{reasons.push(json!({"reason":"insufficient_knowledge_or_valid_candidates","actual_count":n,"target_count":5}));}
  for (ordinal,(p,q,reused)) in candidates.into_iter().enumerate(){let qid=id();
   tx.execute("INSERT INTO questions VALUES(?,?,?,?,?,?,?,?,?,?,?)",params![qid,sid,p.point_id,ordinal,q.prompt,q.reference,q.explanation,serde_json::to_string(&q).map_err(err)?,serde_json::to_string(&p).map_err(err)?,q.fingerprint(),reused]).map_err(err)?;
   let details=json!({"quality_version":"question-v6","rubric_check_version":"rubric-v2","publication_guard":"guard-v3","required":q.required.iter().map(|text|json!({"text":text,"required_for_full_credit":true,"equivalents":"semantic equivalence accepted"})).collect::<Vec<_>>(),"critical_errors":q.critical_errors});
   tx.execute("INSERT INTO review_questions VALUES(?,'grading_standard_v1',?,?,?)",params![qid,p.difficulty,reused.map(|_|"history_repeat_after_rewrite_or_invalid_candidate"),details.to_string()]).map_err(err)?;
  }
  tx.execute("UPDATE sessions SET status='ready',actual_count=?,seq=seq+1 WHERE id=? AND active_run=? AND actual_count=0",params![n,sid,ee.run]).map_err(err)?;
  tx.execute("UPDATE review_sessions SET reasons=? WHERE session_id=?",params![json!(reasons).to_string(),sid]).map_err(err)?;
  Store::finish(&tx,&ee)?;tx.commit().map_err(err)?;Ok(())
 }).await
}
