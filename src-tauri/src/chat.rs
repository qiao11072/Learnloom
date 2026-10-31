//! Durable conversation domain. The base M0 messages table remains the assistant state source.
use crate::{
    app::AppCore,
    hash, id,
    knowledge::{now, Library},
    store::{enqueue_in_transaction, Execution, Store},
    Result,
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Reference {
    pub evidence_id: String,
    pub page_id: String,
    pub revision_id: String,
    pub title: String,
    pub anchor: String,
    pub excerpt: String,
    pub evidence_hash: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    pub id: String,
    pub conversation_id: String,
    pub role: String,
    pub ordinal: i64,
    pub body: String,
    pub references: Vec<Reference>,
    pub status: String,
    pub answer_kind: Option<String>,
    pub job_id: Option<String>,
    pub run_id: Option<String>,
    pub sequence: i64,
    pub created_at: i64,
    pub error: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Update {
    pub kb_id: String,
    pub conversation_id: String,
    pub entity_type: String,
    pub entity_id: String,
    pub job_id: Option<String>,
    pub run_id: Option<String>,
    pub sequence: i64,
    pub kind: String,
    pub payload: Value,
}
pub fn message(db: &Connection, mid: &str) -> Result<Message> {
    if let Some((cid, ord, text, at)) = db
        .query_row(
            "SELECT conversation_id,ordinal,user_text,created_at FROM chat_turns WHERE id=?",
            [mid],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            },
        )
        .optional()
        .map_err(err)?
    {
        return Ok(Message {
            id: mid.into(),
            conversation_id: cid,
            role: "user".into(),
            ordinal: ord * 2,
            body: text,
            references: vec![],
            status: "succeeded".into(),
            answer_kind: None,
            job_id: None,
            run_id: None,
            sequence: 0,
            created_at: at,
            error: String::new(),
        });
    }
    let (cid,ord,body,refs,status,kind,job,run,seq,at,error)=db.query_row("SELECT cm.conversation_id,t.ordinal,m.body,m.refs,m.status,cm.answer_kind,m.job_id,m.active_run,m.seq,cm.created_at,COALESCE(j.error,'') FROM messages m JOIN chat_meta cm ON cm.message_id=m.id JOIN chat_turns t ON t.id=cm.turn_id LEFT JOIN job_meta j ON j.job_id=m.job_id WHERE m.id=?",[mid],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,Option<String>>(5)?,r.get::<_,String>(6)?,r.get::<_,String>(7)?,r.get::<_,i64>(8)?,r.get::<_,i64>(9)?,r.get::<_,String>(10)?))).optional().map_err(err)?.ok_or("MESSAGE_NOT_FOUND")?;
    let succeeded = status == "succeeded";
    Ok(Message {
        id: mid.into(),
        conversation_id: cid,
        role: "assistant".into(),
        ordinal: ord * 2 + 1,
        body,
        references: if succeeded {
            serde_json::from_str(&refs).map_err(err)?
        } else {
            vec![]
        },
        status,
        answer_kind: if succeeded { kind } else { None },
        job_id: Some(job),
        run_id: Some(run),
        sequence: seq,
        created_at: at,
        error,
    })
}
pub fn update(lib: &Library, mid: &str, kind: &str, text: Option<&str>) -> Result<Update> {
    let m = message(&lib.db.conn, mid)?;
    Ok(Update {
        kb_id: lib.marker.id.clone(),
        conversation_id: m.conversation_id.clone(),
        entity_type: "message".into(),
        entity_id: mid.into(),
        job_id: m.job_id.clone(),
        run_id: m.run_id.clone(),
        sequence: m.sequence,
        kind: kind.into(),
        payload: if kind == "delta" {
            json!({"text":text.unwrap_or("")})
        } else {
            serde_json::to_value(m).map_err(err)?
        },
    })
}
pub fn job_update(lib: &Library, jid: &str, kind: &str) -> Result<Option<Update>> {
    let mid:Option<String>=lib.db.conn.query_row("SELECT cm.message_id FROM chat_meta cm JOIN messages m ON m.id=cm.message_id WHERE m.job_id=?",[jid],|r|r.get(0)).optional().map_err(err)?;
    mid.map(|m| update(lib, &m, kind, None)).transpose()
}
pub fn list(lib: &Library, cursor: Option<&str>, limit: usize) -> Result<Value> {
    let limit = limit.clamp(1, 100);
    let (at, id) = if let Some(c) = cursor {
        let (t, id) = c.split_once(':').ok_or("INVALID_CURSOR")?;
        (
            t.parse::<i64>().map_err(|_| "INVALID_CURSOR")?,
            id.to_string(),
        )
    } else {
        (i64::MAX, String::new())
    };
    let mut s=lib.db.conn.prepare("SELECT id,title,updated_at,active_message_id FROM conversations WHERE updated_at<?1 OR(updated_at=?1 AND id>?2) ORDER BY updated_at DESC,id LIMIT ?3").map_err(err)?;
    let mut rows=s.query_map(params![at,id,(limit+1)as i64],|r|Ok(json!({"id":r.get::<_,String>(0)?,"title":r.get::<_,String>(1)?,"updated_at":r.get::<_,i64>(2)?,"active_message_id":r.get::<_,Option<String>>(3)?}))).map_err(err)?.collect::<std::result::Result<Vec<_>,_>>().map_err(err)?;
    let more = rows.len() > limit;
    rows.truncate(limit);
    let next = if more {
        rows.last().map(|r| {
            format!(
                "{}:{}",
                r["updated_at"].as_i64().unwrap(),
                r["id"].as_str().unwrap()
            )
        })
    } else {
        None
    };
    Ok(json!({"items":rows,"next_cursor":next}))
}
pub fn conversation(lib: &Library, cid: &str, before: Option<i64>, limit: usize) -> Result<Value> {
    let tx = lib.db.conn.unchecked_transaction().map_err(err)?;
    let header:Value=tx.query_row("SELECT id,title,updated_at,active_message_id FROM conversations WHERE id=?",[cid],|r|Ok(json!({"id":r.get::<_,String>(0)?,"title":r.get::<_,String>(1)?,"updated_at":r.get::<_,i64>(2)?,"active_message_id":r.get::<_,Option<String>>(3)?}))).optional().map_err(err)?.ok_or("CONVERSATION_NOT_FOUND")?;
    let limit = limit.clamp(1, 50);
    let mut s=tx.prepare("SELECT id,assistant_id,ordinal FROM chat_turns WHERE conversation_id=?1 AND ordinal<?2 ORDER BY ordinal DESC LIMIT ?3").map_err(err)?;
    let mut pairs = s
        .query_map(
            params![cid, before.unwrap_or(i64::MAX), (limit + 1) as i64],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            },
        )
        .map_err(err)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(err)?;
    drop(s);
    let more = pairs.len() > limit;
    pairs.truncate(limit);
    pairs.reverse();
    let next = if more {
        pairs.first().map(|p| p.2)
    } else {
        None
    };
    let mut messages = vec![];
    for (user, assistant, _) in pairs {
        messages.push(message(&tx, &user)?);
        messages.push(message(&tx, &assistant)?);
    }
    tx.commit().map_err(err)?;
    Ok(json!({"conversation":header,"messages":messages,"next_cursor":next,"kb_id":lib.marker.id}))
}
pub fn message_for_job(lib: &Library, e: &Execution) -> Result<String> {
    lib.db.conn.query_row("SELECT m.id FROM messages m JOIN chat_meta cm ON cm.message_id=m.id WHERE m.job_id=? AND m.active_run=?",params![e.job,e.run],|r|r.get(0)).optional().map_err(err)?.ok_or("CONFLICT: 回答已进入其他执行代次".into())
}
pub fn append(lib: &mut Library, e: &Execution, call: &str, text: &str) -> Result<Option<Update>> {
    if text.is_empty() {
        return Ok(None);
    }
    let mid = message_for_job(lib, e)?;
    let tx = lib
        .db
        .conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(err)?;
    if !Store::owns(&tx, e, call)? {
        return Err("CANCELLED".into());
    }
    tx.execute("UPDATE messages SET body=body||?1,status='streaming',seq=seq+1 WHERE id=?2 AND active_run=?3",params![text,mid,e.run]).map_err(err)?;
    tx.execute(
        "UPDATE chat_meta SET updated_at=? WHERE message_id=?",
        params![now(), mid],
    )
    .map_err(err)?;
    tx.commit().map_err(err)?;
    Ok(Some(update(lib, &mid, "delta", Some(text))?))
}
pub fn finish(
    lib: &mut Library,
    e: &Execution,
    call: Option<&str>,
    body: &str,
    kind: &str,
    refs: &[Reference],
) -> Result<Update> {
    let mid = message_for_job(lib, e)?;
    let tx = lib
        .db
        .conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(err)?;
    let owns = if let Some(call) = call {
        Store::owns(&tx, e, call)?
    } else {
        tx.query_row("SELECT EXISTS(SELECT 1 FROM jobs j JOIN runs r ON r.id=j.active_run WHERE j.id=? AND j.active_run=? AND j.status='running' AND r.status='running')",params![e.job,e.run],|r|r.get(0)).map_err(err)?
    };
    if !owns {
        return Err("CANCELLED".into());
    }
    tx.execute(
        "UPDATE messages SET body=?,refs=?,status='succeeded',seq=seq+1 WHERE id=?",
        params![body, serde_json::to_string(refs).map_err(err)?, mid],
    )
    .map_err(err)?;
    tx.execute(
        "UPDATE chat_meta SET answer_kind=?,updated_at=? WHERE message_id=?",
        params![kind, now(), mid],
    )
    .map_err(err)?;
    tx.execute(
        "UPDATE conversations SET active_message_id=NULL,updated_at=? WHERE active_message_id=?",
        params![now(), mid],
    )
    .map_err(err)?;
    tx.execute(
        "UPDATE job_meta SET stage='completed',error='',updated_at=? WHERE job_id=?",
        params![now(), e.job],
    )
    .map_err(err)?;
    Store::finish(&tx, e)?;
    tx.commit().map_err(err)?;
    update(lib, &mid, "replace", None)
}
impl AppCore {
    pub fn ask(self: &Arc<Self>, v: &Value) -> Result<Value> {
        let text = v["text"].as_str().ok_or("INVALID_ARGUMENT")?;
        let key = v["request_key"].as_str().ok_or("INVALID_REQUEST_KEY")?;
        let existing = v["conversation_id"].as_str();
        if text.trim().is_empty() {
            return Err("EMPTY_QUESTION".into());
        }
        if text.len() > 65536 || key.is_empty() || key.len() > 200 {
            return Err("INPUT_TOO_LONG".into());
        }
        let payload = hash(&format!("{}:{text}", existing.unwrap_or("new")));
        let request = format!("chat-request:{key}");
        let (e, kb, cid, uid, mid) = {
            let mut inner = self.inner.lock().map_err(|_| "STATE_LOCK")?;
            let snapshot = Self::model_snapshot(&inner);
            let lib = inner.library.as_mut().ok_or("NO_LIBRARY")?;
            let tx = lib
                .db
                .conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(err)?;
            let old: Option<(String, String)> = tx
                .query_row(
                    "SELECT payload_hash,entity_id FROM request_keys WHERE key=?",
                    [&request],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(err)?;
            if let Some((old, mid)) = old {
                if old != payload {
                    return Err("CONFLICT: 相同请求键的提问内容不同".into());
                }
                let (cid,uid,job,run):(String,String,String,String)=tx.query_row("SELECT cm.conversation_id,cm.turn_id,m.job_id,m.active_run FROM chat_meta cm JOIN messages m ON m.id=cm.message_id WHERE m.id=?",[&mid],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).map_err(err)?;
                return Ok(
                    json!({"conversation_id":cid,"user_message_id":uid,"message_id":mid,"job_id":job,"run_id":run,"duplicate":true}),
                );
            }
            let snapshot = snapshot?;
            let cid = existing.map(str::to_owned).unwrap_or_else(id);
            if existing.is_some() {
                let active: Option<Option<String>> = tx
                    .query_row(
                        "SELECT active_message_id FROM conversations WHERE id=?",
                        [&cid],
                        |r| r.get(0),
                    )
                    .optional()
                    .map_err(err)?;
                match active {
                    None => return Err("CONVERSATION_NOT_FOUND".into()),
                    Some(Some(_)) => return Err("BUSY: 本会话已有生成中的回答".into()),
                    _ => {}
                }
            } else {
                tx.execute(
                    "INSERT INTO conversations VALUES(?1,?2,?3,?3,NULL)",
                    params![
                        cid,
                        text.split_whitespace()
                            .collect::<Vec<_>>()
                            .join(" ")
                            .chars()
                            .take(40)
                            .collect::<String>(),
                        now()
                    ],
                )
                .map_err(err)?;
            }
            let uid = id();
            let mid = id();
            let ordinal: i64 = tx
                .query_row(
                    "SELECT COALESCE(max(ordinal)+1,0) FROM chat_turns WHERE conversation_id=?",
                    [&cid],
                    |r| r.get(0),
                )
                .map_err(err)?;
            let e =
                enqueue_in_transaction(&tx, &format!("ask:{mid}"), "chat", &mid, text, &snapshot)?;
            tx.execute("INSERT INTO messages(id,job_id,active_run,body,refs,status,seq) VALUES(?1,?2,?3,'','[]','queued',0)",params![mid,e.job,e.run]).map_err(err)?;
            tx.execute(
                "INSERT INTO chat_turns VALUES(?1,?2,?3,?4,?5,?6)",
                params![uid, cid, ordinal, text, now(), mid],
            )
            .map_err(err)?;
            tx.execute(
                "INSERT INTO chat_meta VALUES(?1,?2,?3,NULL,?4,?4)",
                params![mid, cid, uid, now()],
            )
            .map_err(err)?;
            tx.execute(
                "INSERT INTO job_meta VALUES(?1,'queued','',?2,?2)",
                params![e.job, now()],
            )
            .map_err(err)?;
            tx.execute(
                "UPDATE conversations SET active_message_id=?,updated_at=? WHERE id=?",
                params![mid, now(), cid],
            )
            .map_err(err)?;
            tx.execute(
                "INSERT INTO request_keys VALUES(?1,?2,?3)",
                params![request, payload, mid],
            )
            .map_err(err)?;
            tx.commit().map_err(err)?;
            (e, lib.marker.id.clone(), cid, uid, mid)
        };
        self.schedule(kb, e.clone());
        Ok(
            json!({"conversation_id":cid,"user_message_id":uid,"message_id":mid,"job_id":e.job,"run_id":e.run,"duplicate":false}),
        )
    }
    pub fn conversations(&self, cursor: Option<&str>, limit: usize) -> Result<Value> {
        let inner = self.inner.lock().map_err(|_| "STATE_LOCK")?;
        list(inner.library.as_ref().ok_or("NO_LIBRARY")?, cursor, limit)
    }
    pub fn conversation(&self, cid: &str, before: Option<i64>, limit: usize) -> Result<Value> {
        let inner = self.inner.lock().map_err(|_| "STATE_LOCK")?;
        conversation(
            inner.library.as_ref().ok_or("NO_LIBRARY")?,
            cid,
            before,
            limit,
        )
    }
    pub fn message(&self, mid: &str) -> Result<Message> {
        let inner = self.inner.lock().map_err(|_| "STATE_LOCK")?;
        message(&inner.library.as_ref().ok_or("NO_LIBRARY")?.db.conn, mid)
    }
}

impl AppCore {
    pub(crate) fn emit(&self, event: Update) {
        let _ = self.updates.send(event);
    }
    pub fn subscribe_chat(
        &self,
        cid: &str,
    ) -> Result<(
        String,
        String,
        tokio::sync::broadcast::Receiver<Update>,
        tokio_util::sync::CancellationToken,
    )> {
        let receiver = self.updates.subscribe();
        let inner = self.inner.lock().map_err(|_| "STATE_LOCK")?;
        let lib = inner.library.as_ref().ok_or("NO_LIBRARY")?;
        let exists: bool = lib
            .db
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM conversations WHERE id=?)",
                [cid],
                |r| r.get(0),
            )
            .map_err(err)?;
        if !exists {
            return Err("CONVERSATION_NOT_FOUND".into());
        }
        let kb = lib.marker.id.clone();
        drop(inner);
        let id = id();
        let token = tokio_util::sync::CancellationToken::new();
        self.subscriptions
            .lock()
            .map_err(|_| "SUBSCRIPTION_LOCK")?
            .insert(id.clone(), token.clone());
        Ok((id, kb, receiver, token))
    }
    pub fn unsubscribe_chat(&self, id: &str) {
        if let Ok(mut subscriptions) = self.subscriptions.lock() {
            if let Some(t) = subscriptions.remove(id) {
                t.cancel();
            }
        }
    }
    pub fn close_subscriptions(&self) {
        if let Ok(mut subscriptions) = self.subscriptions.lock() {
            for (_, token) in std::mem::take(&mut *subscriptions) {
                token.cancel();
            }
        }
    }
}
