use crate::{config::Snapshot, hash, id, Result};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Wiki {
    pub page_id: String,
    pub point_id: String,
    pub title: String,
    pub scope: String,
    pub body: String,
    pub evidence: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Question {
    pub point_id: String,
    pub prompt: String,
    pub reference: String,
    pub explanation: String,
    pub required: Vec<String>,
    pub critical_errors: Vec<String>,
}
impl Question {
    pub fn validate(&self) -> Result<()> {
        if [
            &self.point_id,
            &self.prompt,
            &self.reference,
            &self.explanation,
        ]
        .iter()
        .any(|v| v.trim().is_empty())
            || !(2..=4).contains(&self.required.len())
            || self.required.iter().any(|v| v.trim().is_empty())
        {
            return Err("INVALID_OUTPUT: question".into());
        }
        Ok(())
    }
    pub fn fingerprint(&self) -> String {
        hash(
            &self
                .prompt
                .to_lowercase()
                .split_whitespace()
                .collect::<String>(),
        )
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grade {
    pub score: u8,
    pub matched: Vec<usize>,
    pub missing: Vec<usize>,
    pub critical: Vec<usize>,
    #[serde(default)]
    pub noncritical_errors: Vec<String>,
    pub explanation: String,
    pub standard: String,
}
impl Grade {
    pub fn from_provider(content: &str) -> Result<Self> {
        let mut value: Value = serde_json::from_str(content).map_err(|e| e.to_string())?;
        // Provider boundary only: canonical integer identifier strings carry the
        // same identity. Never coerce scores, booleans, text, or invalid IDs.
        for key in ["matched", "missing", "critical"] {
            if let Some(items) = value[key].as_array_mut() {
                for item in items {
                    if let Some(text) = item.as_str() {
                        if let Ok(number) = text.parse::<u64>() {
                            if number.to_string() == text {
                                *item = json!(number);
                            }
                        }
                    }
                }
            }
        }
        serde_json::from_value(value).map_err(|e| e.to_string())
    }
    pub fn validate(&self, q: &Question) -> Result<()> {
        let covered: std::collections::BTreeSet<_> =
            self.matched.iter().chain(&self.missing).copied().collect();
        let computed = if !self.critical.is_empty() || self.matched.is_empty() {
            0
        } else if !self.missing.is_empty() || !self.noncritical_errors.is_empty() {
            1
        } else {
            2
        };
        if self.score > 2
            || self.standard != "grading_standard_v1"
            || self.explanation.trim().is_empty()
            || self.score != computed
            || covered != (0..q.required.len()).collect()
            || self.matched.iter().any(|i| self.missing.contains(i))
            || covered.len() != self.matched.len() + self.missing.len()
            || self.critical.iter().any(|i| *i >= q.critical_errors.len())
            || (!self.critical.is_empty() && self.score != 0)
            || (self.score == 2
                && (!self.missing.is_empty() || self.matched.len() != q.required.len()))
            || (self.score == 1 && (self.matched.is_empty() || !self.critical.is_empty()))
        {
            return Err("INVALID_OUTPUT: grading standard".into());
        }
        Ok(())
    }
}
pub struct Store {
    pub conn: Connection,
    pub root: PathBuf,
}
#[derive(Clone, Debug)]
pub struct Execution {
    pub job: String,
    pub run: String,
}
fn run_snapshot(snapshot: &Snapshot, kind: &str) -> Result<String> {
    let mut value = serde_json::to_value(snapshot).map_err(err)?;
    if kind == "review" || kind == "grade" {
        value["question_prompt_version"] = json!("question-v6");
        value["question_prompt_hash"] = json!(hash(crate::workflow::QUESTION_PROMPT));
        value["assessment_version"] = json!("assessment-v6-context-v2");
        value["assessment_prompt_hash"] = json!(hash(crate::workflow::GRADE_PROMPT));
        value["grading_standard"] = json!("grading_standard_v1");
        value["policy_version"] = json!("policy_v1");
        value["schema_version"] = json!("review-v1");
    }
    Ok(value.to_string())
}
impl Store {
    pub fn open(root: &Path) -> Result<Self> {
        Self::open_at(root, &root.join("m0.sqlite3"))
    }
    pub fn open_at(root: &Path, database: &Path) -> Result<Self> {
        std::fs::create_dir_all(root).map_err(err)?;
        for name in ["sources", "wiki", "staging"] {
            std::fs::create_dir_all(root.join(name)).map_err(err)?;
        }
        let conn = Connection::open(database).map_err(err)?;
        conn.busy_timeout(std::time::Duration::from_secs(3))
            .map_err(err)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
            .map_err(err)?;
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(err)?;
        if version == 0 {
            conn.execute_batch(&format!(
                "BEGIN IMMEDIATE;{} {} COMMIT;",
                include_str!("../migrations/001_m0.sql"),
                include_str!("../migrations/002_page_bases.sql")
            ))
            .map_err(err)?;
        } else if version == 1 {
            conn.execute_batch(&format!(
                "BEGIN IMMEDIATE;{} COMMIT;",
                include_str!("../migrations/002_page_bases.sql")
            ))
            .map_err(err)?;
        } else if version != 2 {
            return Err("UNSUPPORTED_KB_VERSION".into());
        }
        Ok(Self {
            conn,
            root: root.into(),
        })
    }
    pub fn version(&self) -> Result<i64> {
        self.conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .map_err(err)
    }
    pub fn save_source(&mut self, title: &str, text: &str) -> Result<String> {
        let fingerprint = hash(text);
        if let Some(existing) = self
            .conn
            .query_row("SELECT id FROM sources WHERE hash=?", [&fingerprint], |r| {
                r.get(0)
            })
            .optional()
            .map_err(err)?
        {
            return Ok(existing);
        }
        let sid = id();
        let relative = format!("sources/{sid}.md");
        std::fs::write(self.root.join(&relative), text).map_err(err)?;
        self.conn
            .execute(
                "INSERT INTO sources VALUES(?1,?2,?3,?4)",
                params![sid, title, fingerprint, relative],
            )
            .map_err(err)?;
        Ok(sid)
    }
    pub fn enqueue(
        &mut self,
        key: &str,
        kind: &str,
        entity: &str,
        payload: &str,
        snapshot: &Snapshot,
    ) -> Result<Execution> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(err)?;
        let execution = enqueue_in_transaction(&tx, key, kind, entity, payload, snapshot)?;
        tx.commit().map_err(err)?;
        Ok(execution)
    }
    pub fn snapshot(&self, run: &str) -> Result<Snapshot> {
        let text: String = self
            .conn
            .query_row("SELECT snapshot FROM runs WHERE id=?", [run], |r| r.get(0))
            .map_err(err)?;
        serde_json::from_str(&text).map_err(err)
    }
    pub fn claim(&mut self, e: &Execution) -> Result<()> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(err)?;
        let n=tx.execute("UPDATE jobs SET status='running',seq=seq+1 WHERE id=? AND active_run=? AND status='queued'",params![e.job,e.run]).map_err(err)?;
        if n != 1 {
            return Err("CONFLICT: claim".into());
        }
        tx.execute("UPDATE runs SET status='running' WHERE id=?", [&e.run])
            .map_err(err)?;
        tx.execute("UPDATE attempts SET status='grading' WHERE job_id=? AND active_run=? AND status='pending'",params![e.job,e.run]).map_err(err)?;
        tx.commit().map_err(err)
    }
    pub fn call(&mut self, e: &Execution) -> Result<String> {
        let call = id();
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(err)?;
        let n=tx.execute("UPDATE runs SET active_call=?1 WHERE id=?2 AND status='running' AND EXISTS(SELECT 1 FROM jobs WHERE id=?3 AND active_run=?2 AND status='running')",params![call,e.run,e.job]).map_err(err)?;
        if n != 1 {
            return Err("CONFLICT: call".into());
        }
        // M0 ingest has one target page per job. Pin before its model request.
        tx.execute("INSERT OR IGNORE INTO run_page_bases SELECT ?1,j.entity_id,p.current_revision FROM jobs j LEFT JOIN pages p ON p.id=j.entity_id WHERE j.id=?2 AND j.kind='ingest'",params![e.run,e.job]).map_err(err)?;
        tx.commit().map_err(err)?;
        Ok(call)
    }
    pub fn revoke_call(&mut self, e: &Execution, call: &str) -> Result<()> {
        self.conn
            .execute(
                "UPDATE runs SET active_call=NULL WHERE id=? AND active_call=?",
                params![e.run, call],
            )
            .map_err(err)?;
        Ok(())
    }
    pub(crate) fn owns(tx: &Transaction<'_>, e: &Execution, call: &str) -> Result<bool> {
        tx.query_row("SELECT EXISTS(SELECT 1 FROM jobs j JOIN runs r ON r.id=j.active_run WHERE j.id=? AND j.active_run=? AND j.status='running' AND r.status='running' AND r.active_call=?)",params![e.job,e.run,call],|r|r.get(0)).map_err(err)
    }
    pub(crate) fn finish(tx: &Transaction<'_>, e: &Execution) -> Result<()> {
        tx.execute(
            "UPDATE jobs SET status='succeeded',seq=seq+1 WHERE id=? AND active_run=?",
            params![e.job, e.run],
        )
        .map_err(err)?;
        tx.execute(
            "UPDATE runs SET status='succeeded',active_call=NULL WHERE id=?",
            [&e.run],
        )
        .map_err(err)?;
        Ok(())
    }
    pub fn stop(&mut self, e: &Execution, status: &str) -> Result<bool> {
        if !["cancelled", "failed", "interrupted"].contains(&status) {
            return Err("invalid stop status".into());
        }
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(err)?;
        let n=tx.execute("UPDATE jobs SET status=?1,seq=seq+1 WHERE id=?2 AND active_run=?3 AND status IN('queued','running')",params![status,e.job,e.run]).map_err(err)?;
        if n == 0 {
            return Ok(false);
        }
        tx.execute(
            "UPDATE runs SET status=?,active_call=NULL WHERE id=?",
            params![status, e.run],
        )
        .map_err(err)?;
        tx.execute("UPDATE attempts SET status=?1 WHERE job_id=?2 AND active_run=?3 AND status IN('pending','grading')",params![status,e.job,e.run]).map_err(err)?;
        tx.execute("UPDATE sessions SET status=?1,seq=seq+1 WHERE job_id=?2 AND active_run=?3 AND status='generating'",params![status,e.job,e.run]).map_err(err)?;
        tx.execute(
            "UPDATE messages SET status=?1,seq=seq+1 WHERE job_id=?2 AND active_run=?3",
            params![status, e.job, e.run],
        )
        .map_err(err)?;
        let has_chat: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='chat_meta')",
                [],
                |r| r.get(0),
            )
            .map_err(err)?;
        if has_chat {
            tx.execute("UPDATE conversations SET active_message_id=NULL,updated_at=?1 WHERE active_message_id IN(SELECT m.id FROM messages m JOIN chat_meta cm ON cm.message_id=m.id WHERE m.job_id=?2)",params![crate::knowledge::now(),e.job]).map_err(err)?;
        }
        tx.commit().map_err(err)?;
        Ok(true)
    }
    pub fn retry(&mut self, e: &Execution, key: &str, snapshot: &Snapshot) -> Result<Execution> {
        let payload = hash(&format!("{}:{}", e.job, e.run));
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(err)?;
        if let Some((old, run)) = tx
            .query_row(
                "SELECT payload_hash,entity_id FROM request_keys WHERE key=?",
                [key],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(err)?
        {
            if old != payload {
                return Err("CONFLICT".into());
            }
            return Ok(Execution {
                job: e.job.clone(),
                run,
            });
        }
        let (state, current): (String, String) = tx
            .query_row(
                "SELECT status,active_run FROM jobs WHERE id=?",
                [&e.job],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(err)?;
        if current != e.run || !["failed", "cancelled", "interrupted"].contains(&state.as_str()) {
            return Err("CONFLICT: retry".into());
        }
        let has_chat: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='chat_meta')",
                [],
                |r| r.get(0),
            )
            .map_err(err)?;
        if has_chat {
            let row:Option<(Option<String>,String)>=tx.query_row("SELECT c.active_message_id,cm.message_id FROM chat_meta cm JOIN conversations c ON c.id=cm.conversation_id JOIN messages m ON m.id=cm.message_id WHERE m.job_id=?",[&e.job],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(err)?;
            if let Some((active, message)) = row {
                if active.as_ref().is_some_and(|a| a != &message) {
                    return Err("BUSY: 本会话已有生成中的回答".into());
                }
                tx.execute("UPDATE conversations SET active_message_id=?1,updated_at=?2 WHERE id=(SELECT conversation_id FROM chat_meta WHERE message_id=?1)",params![message,crate::knowledge::now()]).map_err(err)?;
            }
        }
        let kind: String = tx
            .query_row("SELECT kind FROM jobs WHERE id=?", [&e.job], |r| r.get(0))
            .map_err(err)?;
        let run = id();
        let no: i64 = tx
            .query_row(
                "SELECT max(run_no)+1 FROM runs WHERE job_id=?",
                [&e.job],
                |r| r.get(0),
            )
            .map_err(err)?;
        tx.execute(
            "INSERT INTO runs VALUES(?1,?2,?3,'queued',?4,NULL)",
            params![run, e.job, no, run_snapshot(snapshot, &kind)?],
        )
        .map_err(err)?;
        tx.execute(
            "UPDATE jobs SET status='queued',active_run=?,seq=seq+1 WHERE id=?",
            params![run, e.job],
        )
        .map_err(err)?;
        tx.execute(
            "INSERT INTO request_keys VALUES(?1,?2,?3)",
            params![key, payload, run],
        )
        .map_err(err)?;
        tx.execute(
            "UPDATE attempts SET status='pending',active_run=? WHERE job_id=?",
            params![run, e.job],
        )
        .map_err(err)?;
        tx.execute("UPDATE sessions SET status='generating',active_run=?,seq=seq+1 WHERE job_id=? AND actual_count=0",params![run,e.job]).map_err(err)?;
        tx.execute("UPDATE messages SET status='queued',active_run=?,body='',refs='[]',seq=seq+1 WHERE job_id=?",params![run,e.job]).map_err(err)?;
        tx.commit().map_err(err)?;
        Ok(Execution {
            job: e.job.clone(),
            run,
        })
    }
    pub fn recover(&mut self) -> Result<usize> {
        let executions = {
            let mut q = self
                .conn
                .prepare("SELECT id,active_run FROM jobs WHERE status='running'")
                .map_err(err)?;
            let rows = q
                .query_map([], |r| {
                    Ok(Execution {
                        job: r.get(0)?,
                        run: r.get(1)?,
                    })
                })
                .map_err(err)?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(err)?
        };
        for e in &executions {
            self.stop(e, "interrupted")?;
        }
        self.repair_indexes()?;
        Ok(executions.len())
    }
    pub fn publish_wiki(
        &mut self,
        e: &Execution,
        call: &str,
        sid: &str,
        wiki: &Wiki,
        fault: bool,
    ) -> Result<bool> {
        if wiki.body.trim().is_empty()
            || wiki.evidence.trim().is_empty()
            || !wiki.body.contains(&wiki.evidence)
        {
            return Err("INVALID_OUTPUT: evidence".into());
        }
        // Source evidence is checked independently of the model-written body.
        let source_path: String = self
            .conn
            .query_row("SELECT path FROM sources WHERE id=?", [sid], |r| r.get(0))
            .map_err(err)?;
        if !std::fs::read_to_string(self.root.join(source_path))
            .map_err(err)?
            .contains(&wiki.evidence)
        {
            return Err("INVALID_OUTPUT: source grounding".into());
        }
        let revision = id();
        let relative = format!("wiki/{revision}.md");
        std::fs::write(
            self.root.join(&relative),
            format!(
                "---\npage_id: {}\nrevision_id: {revision}\n---\n{}",
                wiki.page_id, wiki.body
            ),
        )
        .map_err(err)?;
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(err)?;
        if !Self::owns(&tx, e, call)? {
            return Ok(false);
        }
        let base: Option<Option<String>> = tx
            .query_row(
                "SELECT revision_id FROM run_page_bases WHERE run_id=? AND page_id=?",
                params![e.run, wiki.page_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(err)?;
        let current: Option<String> = tx
            .query_row(
                "SELECT current_revision FROM pages WHERE id=?",
                [&wiki.page_id],
                |r| r.get(0),
            )
            .optional()
            .map_err(err)?;
        if base.is_none() || base.unwrap() != current {
            return Err("CONFLICT: page base revision".into());
        }
        let existing: Option<(String, String)> = tx
            .query_row(
                "SELECT scope,page_id FROM points WHERE id=?",
                [&wiki.point_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(err)?;
        if existing.is_some_and(|(scope, page)| scope != wiki.scope || page != wiki.page_id) {
            return Err("CONFLICT: knowledge point identity".into());
        }
        tx.execute("INSERT INTO pages VALUES(?1,?2,?3,?3,?3,'ready') ON CONFLICT(id) DO UPDATE SET title=excluded.title,current_revision=excluded.current_revision,indexed_revision=excluded.indexed_revision,index_target=excluded.index_target,index_state='ready'",params![wiki.page_id,wiki.title,revision]).map_err(err)?;
        if fault {
            if std::env::var_os("LEARNLOOM_M0_CRASH_BEFORE_FTS").is_some() {
                std::process::exit(73);
            }
            return Err("INJECTED_BEFORE_FTS".into());
        }
        tx.execute(
            "INSERT INTO revisions VALUES(?1,?2,?3,?4,?5)",
            params![revision, wiki.page_id, relative, hash(&wiki.body), sid],
        )
        .map_err(err)?;
        replace_chunks(&tx, &wiki.page_id, &revision, &wiki.title, &wiki.body)?;
        tx.execute(
            "UPDATE index_tasks SET status='superseded' WHERE page_id=? AND target<>?",
            params![wiki.page_id, revision],
        )
        .map_err(err)?;
        tx.execute("INSERT INTO points VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(id) DO UPDATE SET revision_id=excluded.revision_id,evidence=excluded.evidence",params![wiki.point_id,wiki.title,wiki.scope,wiki.page_id,revision,wiki.evidence]).map_err(err)?;
        Self::finish(&tx, e)?;
        tx.commit().map_err(err)?;
        Ok(true)
    }
    pub fn mark_index_pending(&mut self, page: &str) -> Result<()> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(err)?;
        tx.execute(
            "UPDATE pages SET index_state='pending',index_target=current_revision WHERE id=?",
            [page],
        )
        .map_err(err)?;
        tx.execute("INSERT INTO index_tasks SELECT id,current_revision,'pending' FROM pages WHERE id=? ON CONFLICT(page_id) DO UPDATE SET target=excluded.target,status='pending'",[page]).map_err(err)?;
        tx.commit().map_err(err)
    }
    pub fn repair_indexes(&mut self) -> Result<()> {
        let mut rescheduled = false;
        let pages = {
            let mut q=self.conn.prepare("SELECT p.id,p.current_revision,p.title,r.path,t.target FROM pages p JOIN revisions r ON r.id=p.current_revision JOIN index_tasks t ON t.page_id=p.id WHERE p.index_state IN('pending','failed')").map_err(err)?;
            let rows = q
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                        r.get::<_, String>(4)?,
                    ))
                })
                .map_err(err)?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(err)?
        };
        for (page, rev, title, path, target) in pages {
            if rev != target {
                self.conn
                    .execute(
                        "UPDATE index_tasks SET status='superseded' WHERE page_id=?",
                        [&page],
                    )
                    .map_err(err)?;
                self.mark_index_pending(&page)?;
                rescheduled = true;
                continue;
            }
            let text = std::fs::read_to_string(self.root.join(path)).map_err(err)?;
            let body = text.splitn(3, "---").nth(2).unwrap_or(&text).trim_start();
            let tx = self
                .conn
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(err)?;
            let owns:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM pages p JOIN index_tasks t ON t.page_id=p.id WHERE p.id=?1 AND p.current_revision=?2 AND p.index_state IN('pending','failed') AND t.target=?2)",params![page,rev],|r|r.get(0)).map_err(err)?;
            if !owns {
                tx.rollback().map_err(err)?;
                continue;
            }
            replace_chunks(&tx, &page, &rev, &title, body)?;
            tx.execute("UPDATE pages SET indexed_revision=?1,index_state='ready' WHERE id=?2 AND current_revision=?1",params![rev,page]).map_err(err)?;
            tx.execute(
                "UPDATE index_tasks SET status='succeeded' WHERE page_id=? AND target=?",
                params![page, rev],
            )
            .map_err(err)?;
            tx.commit().map_err(err)?;
        }
        if rescheduled {
            self.repair_indexes()?;
        }
        Ok(())
    }
    pub fn search(&self, query: &str) -> Result<Vec<String>> {
        if query.chars().count() >= 3
            && query
                .chars()
                .any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
        {
            // M0 correction: whole-sentence phrase matching had only 20/30 recall.
            // For CJK questions use bounded bigram lexical ranking over current
            // chunks. Keep the same revision/index-state guard as FTS queries.
            let chars: Vec<_> = query.chars().collect();
            let terms: std::collections::BTreeSet<String> = chars
                .windows(2)
                .filter(|w| w.iter().all(|c| ('\u{4e00}'..='\u{9fff}').contains(c)))
                .map(|w| w.iter().collect())
                .collect();
            let mut stmt=self.conn.prepare("SELECT p.id,p.title,c.text FROM pages p JOIN chunks c ON c.page_id=p.id WHERE p.index_state='ready' AND p.current_revision=c.revision_id AND p.indexed_revision=c.revision_id LIMIT 10000").map_err(err)?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })
                .map_err(err)?;
            let mut scores = std::collections::BTreeMap::<String, usize>::new();
            for row in rows {
                let (page, title, text) = row.map_err(err)?;
                let score = if title.contains(query) { 200 } else { 0 }
                    + terms
                        .iter()
                        .map(|term| {
                            5 * usize::from(title.contains(term)) + usize::from(text.contains(term))
                        })
                        .sum::<usize>();
                if score > 0 {
                    scores
                        .entry(page)
                        .and_modify(|s| *s = (*s).max(score))
                        .or_insert(score);
                }
            }
            let mut ranked: Vec<_> = scores.into_iter().collect();
            ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            return Ok(ranked.into_iter().take(5).map(|r| r.0).collect());
        }
        let sql = if query.chars().count() < 3 {
            "SELECT DISTINCT p.id FROM pages p JOIN chunks c ON c.page_id=p.id WHERE p.index_state='ready' AND p.current_revision=c.revision_id AND p.indexed_revision=c.revision_id AND (c.text LIKE ?1 OR p.title LIKE ?1) LIMIT 5"
        } else {
            "SELECT DISTINCT p.id FROM page_fts f JOIN chunks c ON c.rowid=f.rowid JOIN pages p ON p.id=c.page_id WHERE page_fts MATCH ?1 AND p.index_state='ready' AND p.current_revision=c.revision_id AND p.indexed_revision=c.revision_id LIMIT 5"
        };
        let arg = if query.chars().count() < 3 {
            format!("%{}%", query.replace('%', ""))
        } else {
            format!("\"{}\"", query.replace('"', "\"\""))
        };
        let mut q = self.conn.prepare(sql).map_err(err)?;
        let rows = q.query_map([arg], |r| r.get(0)).map_err(err)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(err)
    }
    pub fn session(&mut self, date: &str) -> Result<String> {
        self.conn
            .execute(
                "INSERT OR IGNORE INTO sessions(id,date,status) VALUES(?1,?2,'generating')",
                params![id(), date],
            )
            .map_err(err)?;
        self.conn
            .query_row("SELECT id FROM sessions WHERE date=?", [date], |r| r.get(0))
            .map_err(err)
    }
    pub fn bind_session(&mut self, session: &str, e: &Execution) -> Result<()> {
        self.conn
            .execute(
                "UPDATE sessions SET job_id=?,active_run=? WHERE id=? AND actual_count=0",
                params![e.job, e.run, session],
            )
            .map_err(err)?;
        Ok(())
    }
    pub fn publish_questions(
        &mut self,
        e: &Execution,
        call: &str,
        session: &str,
        candidates: &[Question],
    ) -> Result<Vec<String>> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(err)?;
        if !Self::owns(&tx, e, call)? {
            return Err("STALE_RUN".into());
        }
        let bound:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM sessions WHERE id=? AND active_run=? AND actual_count=0 AND status='generating')",params![session,e.run],|r|r.get(0)).map_err(err)?;
        if !bound {
            return Err("CONFLICT: session".into());
        }
        let mut ids = vec![];
        let mut used = std::collections::BTreeSet::new();
        let mut fingerprints = std::collections::BTreeSet::new();
        for q in candidates {
            if q.validate().is_err()
                || used.contains(&q.point_id)
                || fingerprints.contains(&q.fingerprint())
            {
                continue;
            }
            let evidence:Option<(String,String,String)>=tx.query_row("SELECT p.page_id,p.revision_id,p.evidence FROM points p JOIN pages w ON w.id=p.page_id WHERE p.id=? AND p.revision_id=w.current_revision AND w.index_state='ready'",[&q.point_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(err)?;
            let Some((page, rev, text)) = evidence else {
                continue;
            };
            let snapshot = json!({"page_id":page,"revision_id":rev,"evidence":text});
            let reused:Option<String>=tx.query_row("SELECT id FROM questions WHERE point_id=? AND fingerprint=? AND snapshot=? ORDER BY rowid ASC LIMIT 1",params![q.point_id,q.fingerprint(),snapshot.to_string()],|r|r.get(0)).optional().map_err(err)?;
            let question = id();
            tx.execute(
                "INSERT INTO questions VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                params![
                    question,
                    session,
                    q.point_id,
                    ids.len(),
                    q.prompt,
                    q.reference,
                    q.explanation,
                    serde_json::to_string(q).map_err(err)?,
                    snapshot.to_string(),
                    q.fingerprint(),
                    reused
                ],
            )
            .map_err(err)?;
            used.insert(q.point_id.clone());
            fingerprints.insert(q.fingerprint());
            ids.push(question);
        }
        if ids.is_empty() {
            return Err("NO_VALID_QUESTION".into());
        }
        tx.execute(
            "UPDATE sessions SET status='ready',actual_count=?,seq=seq+1 WHERE id=?",
            params![ids.len(), session],
        )
        .map_err(err)?;
        Self::finish(&tx, e)?;
        tx.commit().map_err(err)?;
        Ok(ids)
    }
    pub fn submit(
        &mut self,
        question: &str,
        key: &str,
        answer: &str,
        submitted: &str,
        snapshot: &Snapshot,
    ) -> Result<(String, Execution)> {
        if answer.trim().is_empty() {
            return Err("EMPTY_ANSWER".into());
        }
        let payload = hash(&format!("{question}\n{answer}"));
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(err)?;
        if let Some((aid,jid,run,old))=tx.query_row("SELECT a.id,a.job_id,a.active_run,a.request_hash FROM attempts a WHERE question_id=? OR submission_key=?",params![question,key],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?))).optional().map_err(err)? {
            if old!=payload {return Err("CONFLICT".into());}return Ok((aid,Execution{job:jid,run}));
        }
        let attempt = id();
        let e = enqueue_in_transaction(
            &tx,
            &format!("grade:{attempt}"),
            "grade",
            &attempt,
            answer,
            snapshot,
        )?;
        tx.execute("INSERT INTO attempts(id,question_id,submission_key,request_hash,answer,submitted_at,status,active_run,job_id) VALUES(?1,?2,?3,?4,?5,?6,'pending',?7,?8)",params![attempt,question,key,payload,answer,submitted,e.run,e.job]).map_err(err)?;
        refresh_sessions(&tx)?;
        tx.commit().map_err(err)?;
        Ok((attempt, e))
    }
    pub fn grade(
        &mut self,
        e: &Execution,
        call: &str,
        attempt: &str,
        g: &Grade,
        graded_at: &str,
    ) -> Result<bool> {
        self.grade_recorded(e, call, attempt, g, graded_at, None)
    }
    pub fn grade_recorded(
        &mut self,
        e: &Execution,
        call: &str,
        attempt: &str,
        g: &Grade,
        graded_at: &str,
        assessment: Option<&str>,
    ) -> Result<bool> {
        let qtext:String=self.conn.query_row("SELECT q.rubric FROM questions q JOIN attempts a ON a.question_id=q.id WHERE a.id=?",[attempt],|r|r.get(0)).map_err(err)?;
        let q: Question = serde_json::from_str(&qtext).map_err(err)?;
        g.validate(&q)?;
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(err)?;
        if !Self::owns(&tx, e, call)? {
            return Ok(false);
        }
        let n=tx.execute("UPDATE attempts SET status='graded',score=?1,feedback=?2,graded_at=?3 WHERE id=?4 AND active_run=?5 AND status='grading'",params![g.score,serde_json::to_string(g).map_err(err)?,graded_at,attempt,e.run]).map_err(err)?;
        if n != 1 {
            return Ok(false);
        }
        crate::mastery::project(&tx,&q.point_id)?;
        if let Some(raw) = assessment {
            tx.execute(
                "INSERT INTO grading_assessments VALUES(?,?,?)",
                params![attempt, e.run, raw],
            )
            .map_err(err)?;

        }
        refresh_sessions(&tx)?;
        Self::finish(&tx, e)?;
        tx.commit().map_err(err)?;
        Ok(true)
    }
    pub fn selection(&self) -> Result<Vec<String>> {
        let mut q=self.conn.prepare("SELECT p.id FROM points p LEFT JOIN mastery m ON m.point_id=p.id ORDER BY CASE WHEN m.weak=1 THEN 0 WHEN m.point_id IS NULL THEN 1 ELSE 2 END,coalesce(m.recent,0),p.id").map_err(err)?;
        let rows = q.query_map([], |r| r.get(0)).map_err(err)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(err)
    }
    pub fn append_message(
        &mut self,
        e: &Execution,
        call: &str,
        message: &str,
        text: &str,
    ) -> Result<bool> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(err)?;
        if !Self::owns(&tx, e, call)? {
            return Ok(false);
        }
        tx.execute("INSERT INTO messages VALUES(?1,?2,?3,?4,'[]','streaming',1) ON CONFLICT(id) DO UPDATE SET body=body||excluded.body,status='streaming',seq=seq+1 WHERE active_run=excluded.active_run",params![message,e.job,e.run,text]).map_err(err)?;
        tx.commit().map_err(err)?;
        Ok(true)
    }
    pub fn message(&self, message: &str) -> Result<Value> {
        self.conn.query_row("SELECT body,refs,status,active_run,seq FROM messages WHERE id=?",[message],|r|Ok(json!({"body":r.get::<_,String>(0)?,"references":serde_json::from_str::<Value>(&r.get::<_,String>(1)?).unwrap(),"status":r.get::<_,String>(2)?,"run_id":r.get::<_,String>(3)?,"sequence":r.get::<_,i64>(4)?}))).map_err(err)
    }
}
pub(crate) fn enqueue_in_transaction(
    tx: &Transaction<'_>,
    key: &str,
    kind: &str,
    entity: &str,
    payload: &str,
    snapshot: &Snapshot,
) -> Result<Execution> {
    let payload_hash = hash(payload);
    if let Some((job, run, old_hash)) = tx
        .query_row(
            "SELECT id,active_run,request_hash FROM jobs WHERE business_key=?",
            [key],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            },
        )
        .optional()
        .map_err(err)?
    {
        if payload_hash != old_hash {
            return Err("CONFLICT".into());
        }
        return Ok(Execution { job, run });
    }
    let execution = Execution {
        job: id(),
        run: id(),
    };
    tx.execute("INSERT INTO jobs(id,business_key,request_hash,kind,entity_id,status,active_run) VALUES(?1,?2,?3,?4,?5,'queued',?6)",params![execution.job,key,payload_hash,kind,entity,execution.run]).map_err(err)?;
    tx.execute(
        "INSERT INTO runs VALUES(?1,?2,1,'queued',?3,NULL)",
        params![execution.run, execution.job, run_snapshot(snapshot, kind)?],
    )
    .map_err(err)?;
    Ok(execution)
}
fn refresh_sessions(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch("UPDATE sessions SET status=CASE WHEN (SELECT count(*) FROM attempts a JOIN questions q ON q.id=a.question_id WHERE q.session_id=sessions.id AND a.status='graded')=actual_count THEN 'completed' WHEN (SELECT count(*) FROM attempts a JOIN questions q ON q.id=a.question_id WHERE q.session_id=sessions.id)=actual_count THEN 'awaiting_grading' ELSE 'in_progress' END,seq=seq+1 WHERE actual_count>0 AND EXISTS(SELECT 1 FROM attempts a JOIN questions q ON q.id=a.question_id WHERE q.session_id=sessions.id);").map_err(err)
}
pub(crate) fn replace_chunks(
    tx: &Transaction<'_>,
    page: &str,
    revision: &str,
    title: &str,
    body: &str,
) -> Result<()> {
    tx.execute(
        "DELETE FROM page_fts WHERE rowid IN(SELECT rowid FROM chunks WHERE page_id=?)",
        [page],
    )
    .map_err(err)?;
    tx.execute("DELETE FROM chunks WHERE page_id=?", [page])
        .map_err(err)?;
    let chars: Vec<_> = body.chars().collect();
    let has_meta: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='chunk_meta')",
            [],
            |r| r.get(0),
        )
        .map_err(err)?;
    let mut start = 0;
    let mut ordinal = 0;
    let mut heading = title.to_string();
    while start < chars.len() {
        let mut end = (start + 2000).min(chars.len());
        if end < chars.len() {
            // Prefer a paragraph/line boundary without making tiny chunks.
            if let Some(boundary) = (start + 1000..end).rev().find(|i| chars[*i] == '\n') {
                end = boundary + 1;
            }
        }
        let text: String = chars[start..end].iter().collect();
        tx.execute(
            "INSERT INTO chunks(page_id,revision_id,ordinal,text) VALUES(?1,?2,?3,?4)",
            params![page, revision, ordinal, text],
        )
        .map_err(err)?;
        let rowid = tx.last_insert_rowid();
        if has_meta {
            if let Some(h) = text.lines().find(|l| l.starts_with('#')) {
                heading = h.trim_start_matches('#').trim().into();
            }
            tx.execute(
                "INSERT INTO chunk_meta VALUES(?1,?2,?3,'paragraph-v1')",
                params![rowid, heading, hash(&text)],
            )
            .map_err(err)?;
        }
        tx.execute(
            "INSERT INTO page_fts(rowid,title,body) VALUES(?1,?2,?3)",
            params![rowid, title, text],
        )
        .map_err(err)?;
        if end == chars.len() {
            break;
        }
        start = end - 200;
        ordinal += 1;
    }
    Ok(())
}
