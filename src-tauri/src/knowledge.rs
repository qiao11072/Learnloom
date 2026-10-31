//! Formal knowledge base storage, reusing M0 transaction and run-fencing primitives.
use crate::{
    hash, id,
    store::{replace_chunks, Execution, Store},
    Result,
};
use fs2::FileExt;
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension(format!("{}.tmp", id()));
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .map_err(err)?;
    f.write_all(bytes).map_err(err)?;
    f.sync_all().map_err(err)?;
    drop(f);
    fs::rename(&tmp, path).map_err(err)
}
pub fn no_reparse(path: &Path) -> Result<()> {
    if let Ok(meta) = fs::symlink_metadata(path) {
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if meta.file_attributes() & 0x400 != 0 {
                return Err("UNSAFE_PATH: 不支持知识库内部的链接或junction".into());
            }
        }
        if meta.file_type().is_symlink() {
            return Err("UNSAFE_PATH: symbolic link".into());
        }
    }
    Ok(())
}
pub fn safe_file(root: &Path, relative: &str) -> Result<PathBuf> {
    let p = Path::new(relative);
    if p.is_absolute()
        || p.components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err("UNSAFE_PATH".into());
    }
    let mut target = root.to_path_buf();
    for part in p.components() {
        target.push(part);
        no_reparse(&target)?;
    }
    Ok(target)
}
pub fn normalize_location(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return fs::canonicalize(path).map_err(err);
    }
    let name = path.file_name().ok_or("INVALID_PATH")?;
    if name.to_string_lossy().trim().is_empty() {
        return Err("INVALID_PATH".into());
    }
    let parent = path.parent().ok_or("INVALID_PATH")?;
    if !parent.exists() {
        return Err("PATH_PARENT_MISSING".into());
    }
    Ok(fs::canonicalize(parent).map_err(err)?.join(name))
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Marker {
    pub format_version: u32,
    pub id: String,
    pub name: String,
}
pub fn inspect(path: &Path) -> Result<Value> {
    let path = normalize_location(path)?;
    if path.exists() && !path.is_dir() {
        return Err("NOT_A_DIRECTORY".into());
    }
    let marker = path.join("learnloom.json");
    no_reparse(&marker)?;
    let kind = if marker.exists() {
        let m: Marker = serde_json::from_slice(&fs::read(&marker).map_err(err)?)
            .map_err(|_| "INVALID_KB_MARKER")?;
        if m.format_version != 1 {
            return Err("UNSUPPORTED_KB_VERSION".into());
        }
        uuid::Uuid::parse_str(&m.id).map_err(|_| "INVALID_KB_MARKER")?;
        for child in [".learnloom", "sources", "wiki", "staging"] {
            no_reparse(&path.join(child))?;
        }
        "learnloom"
    } else if !path.exists() || fs::read_dir(&path).map_err(err)?.next().is_none() {
        "empty"
    } else if path.join("schema.md").exists() {
        "external"
    } else {
        "non_empty"
    };
    if ["empty", "learnloom"].contains(&kind) && path.exists() {
        let probe = path.join(format!(".write-check-{}", id()));
        let f = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&probe)
            .map_err(|_| "PATH_NOT_WRITABLE")?;
        f.sync_all().map_err(err)?;
        drop(f);
        fs::remove_file(probe).map_err(err)?;
    }
    Ok(json!({"path":path,"kind":kind,"can_create":kind=="empty","can_open":kind=="learnloom"}))
}
pub struct Library {
    pub marker: Marker,
    pub db: Store,
    pub root: PathBuf,
    _lock: File,
}
impl Library {
    pub fn open(path: &Path, name: &str, create: bool) -> Result<Self> {
        let info = inspect(path)?;
        let root = PathBuf::from(info["path"].as_str().ok_or("INVALID_PATH")?);
        if create && info["kind"] != "empty" {
            return Err("EXISTING_CONTENT: 目录非空，请打开已有知识库或另选空目录".into());
        }
        if !create && info["kind"] != "learnloom" {
            return Err("NOT_A_LEARNLOOM_KB".into());
        }
        if !create && !root.join(".learnloom/learnloom.sqlite3").is_file() {
            return Err("KB_DATABASE_MISSING: 知识库数据库缺失，请保留原始文件并从备份恢复；不会自动重建或清理原文".into());
        }
        if create && (name.trim().is_empty() || name.chars().count() > 120) {
            return Err("INVALID_NAME".into());
        }
        fs::create_dir_all(&root).map_err(err)?;
        let private = root.join(".learnloom");
        no_reparse(&private)?;
        fs::create_dir_all(&private).map_err(err)?;
        let lockpath = private.join("write.lock");
        no_reparse(&lockpath)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lockpath)
            .map_err(err)?;
        lock.try_lock_exclusive()
            .map_err(|_| "KB_ALREADY_OPEN: 此知识库正在被另一个实例使用")?;
        let markerpath = root.join("learnloom.json");
        let marker = if create {
            let m = Marker {
                format_version: 1,
                id: id(),
                name: name.trim().into(),
            };
            atomic_write(&markerpath, &serde_json::to_vec_pretty(&m).map_err(err)?)?;
            m
        } else {
            serde_json::from_slice(&fs::read(&markerpath).map_err(err)?).map_err(err)?
        };
        for dir in [
            "sources",
            "wiki",
            "staging",
            ".learnloom/backups",
            ".learnloom/checkpoints",
        ] {
            let p = safe_file(&root, dir)?;
            fs::create_dir_all(p).map_err(err)?;
        }
        let dbpath = safe_file(&root, ".learnloom/learnloom.sqlite3")?;
        let db = Store::open_at(&root, &dbpath)?;
        let exists: bool = db
            .conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='m1_migrations')",
                [],
                |r| r.get(0),
            )
            .map_err(err)?;
        if !exists {
            let backup = private
                .join("backups")
                .join(format!("before-m1-{}.sqlite3", id()));
            db.conn.backup("main", &backup, None).map_err(err)?;
            db.conn
                .execute_batch(&format!(
                    "BEGIN IMMEDIATE;{} COMMIT;",
                    include_str!("../migrations/003_knowledge.sql")
                ))
                .map_err(err)?;
        } else {
            let v: i64 = db
                .conn
                .query_row("SELECT max(version) FROM m1_migrations", [], |r| r.get(0))
                .map_err(err)?;
            if v > 5 || v < 1 {
                return Err("UNSUPPORTED_KB_VERSION".into());
            }
        }
        let version: i64 = db
            .conn
            .query_row("SELECT max(version) FROM m1_migrations", [], |r| r.get(0))
            .map_err(err)?;
        if version < 2 {
            db.conn
                .backup(
                    "main",
                    private
                        .join("backups")
                        .join(format!("before-m1-2-{}.sqlite3", id())),
                    None,
                )
                .map_err(err)?;
            db.conn
                .execute_batch(&format!(
                    "BEGIN IMMEDIATE;{} COMMIT;",
                    include_str!("../migrations/004_revision_evidence.sql")
                ))
                .map_err(err)?;
        }
        if version < 3 {
            db.conn
                .backup(
                    "main",
                    private
                        .join("backups")
                        .join(format!("before-m2-{}.sqlite3", id())),
                    None,
                )
                .map_err(err)?;
            db.conn
                .execute_batch(&format!(
                    "BEGIN IMMEDIATE;{} COMMIT;",
                    include_str!("../migrations/005_chat.sql")
                ))
                .map_err(err)?;
        }
        if version < 4 {
            db.conn
                .backup(
                    "main",
                    private
                        .join("backups")
                        .join(format!("before-m3-{}.sqlite3", id())),
                    None,
                )
                .map_err(err)?;
            db.conn
                .execute_batch(&format!(
                    "BEGIN IMMEDIATE;{} COMMIT;",
                    include_str!("../migrations/006_review.sql")
                ))
                .map_err(err)?;
        }
        if version < 5 {
            db.conn.backup("main", private.join("backups").join(format!("before-chat-supplement-{}.sqlite3", id())), None).map_err(err)?;
            db.conn.execute_batch(&format!("BEGIN IMMEDIATE;{} COMMIT;", include_str!("../migrations/007_chat_supplement.sql"))).map_err(err)?;
        }
        Ok(Self {
            marker,
            db,
            root,
            _lock: lock,
        })
    }
    pub fn recover(&mut self) -> Result<usize> {
        // Missing/corrupt current content is never used as model evidence.
        let invalid = {
            let mut st=self.db.conn.prepare("SELECT r.path,r.hash,p.id FROM pages p JOIN revisions r ON r.id=p.current_revision").map_err(err)?;
            let rows = st
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })
                .map_err(err)?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(err)?
        };
        for (path, expected, page) in invalid {
            let ok = safe_file(&self.root, &path)
                .ok()
                .and_then(|p| fs::read_to_string(p).ok())
                .map(|s| hash(body_only(&s)) == expected)
                .unwrap_or(false);
            if !ok {
                self.db
                    .conn
                    .execute(
                        "UPDATE pages SET index_state='unavailable' WHERE id=?",
                        [page],
                    )
                    .map_err(err)?;
            }
        }
        // A chunker migration is a recoverable index task; never expose incomplete metadata.
        let missing_meta = {
            let mut s=self.db.conn.prepare("SELECT DISTINCT p.id FROM pages p JOIN chunks c ON c.page_id=p.id LEFT JOIN chunk_meta m ON m.chunk_rowid=c.rowid WHERE p.index_state='ready' AND m.chunk_rowid IS NULL").map_err(err)?;
            let rows = s
                .query_map([], |r| r.get::<_, String>(0))
                .map_err(err)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(err)?;
            rows
        };
        for pid in missing_meta {
            self.db.mark_index_pending(&pid)?;
        }
        // Recover pure local pending indexes before queued jobs resume.
        let count = self.db.recover()?;
        // Restoring a conversation must never automatically repeat a paid request.
        let queued = {
            let mut s=self.db.conn.prepare("SELECT j.id,j.active_run FROM jobs j WHERE j.kind IN('chat','review','grade') AND j.status='queued'").map_err(err)?;
            let rows = s
                .query_map([], |r| {
                    Ok(Execution {
                        job: r.get(0)?,
                        run: r.get(1)?,
                    })
                })
                .map_err(err)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(err)?;
            rows
        };
        for e in queued {
            self.db.stop(&e, "interrupted")?;
        }
        self.db.conn.execute("UPDATE job_meta SET stage='interrupted',error='应用上次运行中断，可显式重试',updated_at=? WHERE job_id IN(SELECT id FROM jobs WHERE status='interrupted')",[now()]).map_err(err)?;
        let missing:bool=self.db.conn.query_row("SELECT EXISTS(SELECT 1 FROM pages p WHERE p.index_state='ready' AND NOT EXISTS(SELECT 1 FROM page_terms t WHERE t.page_id=p.id AND t.revision_id=p.current_revision))",[],|r|r.get(0)).map_err(err)?;
        if missing {
            self.rebuild_terms()?;
        }
        crate::mastery::rebuild(self)?;
        self.gc_orphans()?;
        Ok(count)
    }
    pub fn gc_orphans(&self) -> Result<()> {
        // Only app-owned immutable revision/checkpoint files; source originals stay intact.
        let known = {
            let mut s = self
                .db
                .conn
                .prepare("SELECT path FROM revisions UNION SELECT artifact_path FROM checkpoints")
                .map_err(err)?;
            let rows = s.query_map([], |r| r.get::<_, String>(0)).map_err(err)?;
            rows.collect::<std::result::Result<std::collections::BTreeSet<_>, _>>()
                .map_err(err)?
        };
        for folder in ["wiki", ".learnloom/checkpoints"] {
            for file in fs::read_dir(safe_file(&self.root, folder)?).map_err(err)? {
                let file = file.map_err(err)?;
                let p = file.path();
                no_reparse(&p)?;
                let rel = p
                    .strip_prefix(&self.root)
                    .map_err(err)?
                    .to_string_lossy()
                    .replace('\\', "/");
                let file_name = p.file_name().unwrap_or_default().to_string_lossy();
                let owned = if folder == "wiki" {
                    file_name
                        .strip_suffix(".md")
                        .is_some_and(|n| uuid::Uuid::parse_str(n).is_ok())
                } else {
                    file_name
                        .strip_suffix(".json")
                        .is_some_and(|n| n.len() == 64 && n.chars().all(|c| c.is_ascii_hexdigit()))
                };
                if p.is_file() && owned && !known.contains(&rel) {
                    fs::remove_file(p).map_err(err)?;
                }
            }
        }
        Ok(())
    }
    pub fn save_source(&mut self, title: &str, text: &str, kind: &str, key: &str) -> Result<Value> {
        if key.is_empty() || key.len() > 200 {
            return Err("INVALID_REQUEST_KEY".into());
        }
        if title.trim().is_empty() || title.chars().count() > 200 || text.trim().is_empty() {
            return Err("INVALID_SOURCE: 标题和资料正文不能为空".into());
        }
        if text.len() > 16 * 1024 * 1024 {
            return Err("SOURCE_TOO_LARGE: 原始资料上限16MB".into());
        }
        let fingerprint = hash(text);
        let payload = hash(&format!("{title}\n{fingerprint}"));
        let tx = self
            .db
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(err)?;
        if let Some((old, sid)) = tx
            .query_row(
                "SELECT payload_hash,entity_id FROM request_keys WHERE key=?",
                [key],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(err)?
        {
            if old != payload {
                return Err("CONFLICT: 相同请求键的内容发生变化".into());
            }
            return Ok(json!({"source_id":sid,"duplicate":true}));
        }
        if let Some(sid) = tx
            .query_row("SELECT id FROM sources WHERE hash=?", [&fingerprint], |r| {
                r.get::<_, String>(0)
            })
            .optional()
            .map_err(err)?
        {
            tx.execute(
                "INSERT INTO request_keys VALUES(?1,?2,?3)",
                params![key, payload, sid],
            )
            .map_err(err)?;
            tx.commit().map_err(err)?;
            return Ok(json!({"source_id":sid,"duplicate":true}));
        }
        let sid = id();
        let relative = format!("sources/{sid}.md");
        let target = safe_file(&self.root, &relative)?;
        atomic_write(&target, text.as_bytes())?;
        tx.execute(
            "INSERT INTO sources VALUES(?1,?2,?3,?4)",
            params![sid, title.trim(), fingerprint, relative],
        )
        .map_err(err)?;
        tx.execute(
            "INSERT INTO source_meta VALUES(?1,?2,?3)",
            params![sid, now(), kind],
        )
        .map_err(err)?;
        tx.execute(
            "INSERT INTO request_keys VALUES(?1,?2,?3)",
            params![key, payload, sid],
        )
        .map_err(err)?;
        tx.commit().map_err(err)?;
        Ok(json!({"source_id":sid,"duplicate":false}))
    }
    pub fn source(&self, sid: &str) -> Result<Value> {
        let (title, path, expected): (String, String, String) = self
            .db
            .conn
            .query_row(
                "SELECT title,path,hash FROM sources WHERE id=?",
                [sid],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map_err(err)?;
        let body = fs::read_to_string(safe_file(&self.root, &path)?).map_err(err)?;
        if hash(&body) != expected {
            return Err("SOURCE_CORRUPTED: 原始资料校验失败".into());
        }
        Ok(json!({"id":sid,"title":title,"body":body,"hash":expected}))
    }
    pub fn page(&self, pid: &str, revision: Option<&str>) -> Result<Value> {
        let (title, current, state): (String, String, String) = self
            .db
            .conn
            .query_row(
                "SELECT title,current_revision,index_state FROM pages WHERE id=?",
                [pid],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map_err(err)?;
        let rev = revision.unwrap_or(&current);
        let (path, expected): (String, String) = self
            .db
            .conn
            .query_row(
                "SELECT path,hash FROM revisions WHERE id=? AND page_id=?",
                params![rev, pid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(err)?;
        let raw = fs::read_to_string(safe_file(&self.root, &path)?).map_err(err)?;
        let body = body_only(&raw);
        if hash(body) != expected {
            return Err("PAGE_CORRUPTED".into());
        }
        let mut s=self.db.conn.prepare("SELECT s.id,s.title FROM source_pages ps JOIN sources s ON s.id=ps.source_id WHERE ps.page_id=? AND ps.revision_id=?").map_err(err)?;
        let sources = s
            .query_map(params![pid, rev], |r| {
                Ok(json!({"id":r.get::<_,String>(0)?,"title":r.get::<_,String>(1)?}))
            })
            .map_err(err)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(err)?;
        let mut s=self.db.conn.prepare("SELECT p.id,p.title FROM page_links l JOIN pages p ON p.id=l.to_page WHERE l.from_page=? AND l.from_revision=?").map_err(err)?;
        let links = s
            .query_map(params![pid, rev], |r| {
                Ok(json!({"id":r.get::<_,String>(0)?,"title":r.get::<_,String>(1)?}))
            })
            .map_err(err)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(err)?;
        let mut s = self
            .db
            .conn
            .prepare("SELECT v.point_id,v.name,v.evidence FROM point_versions v JOIN points p ON p.id=v.point_id WHERE p.page_id=? AND v.revision_id=?")
            .map_err(err)?;
        let points=s.query_map(params![pid,rev],|r|Ok(json!({"id":r.get::<_,String>(0)?,"name":r.get::<_,String>(1)?,"evidence":r.get::<_,String>(2)?}))).map_err(err)?.collect::<std::result::Result<Vec<_>,_>>().map_err(err)?;
        Ok(
            json!({"id":pid,"title":title,"revision":rev,"current_revision":current,"index_state":state,"body":body,"sources":sources,"links":links,"points":points}),
        )
    }
    pub fn lists(&self) -> Result<Value> {
        let mut s=self.db.conn.prepare("SELECT s.id,s.title,m.created_at,COALESCE(j.status,'saved'),COALESCE(j.id,''),COALESCE(j.active_run,''),COALESCE(jm.stage,''),COALESCE(jm.error,'') FROM sources s JOIN source_meta m ON m.source_id=s.id LEFT JOIN jobs j ON j.entity_id=s.id AND j.kind='organize' LEFT JOIN job_meta jm ON jm.job_id=j.id ORDER BY m.created_at DESC").map_err(err)?;
        let sources=s.query_map([],|r|Ok(json!({"id":r.get::<_,String>(0)?,"title":r.get::<_,String>(1)?,"created_at":r.get::<_,i64>(2)?,"status":r.get::<_,String>(3)?,"job_id":r.get::<_,String>(4)?,"run_id":r.get::<_,String>(5)?,"stage":r.get::<_,String>(6)?,"error":r.get::<_,String>(7)?}))).map_err(err)?.collect::<std::result::Result<Vec<_>,_>>().map_err(err)?;
        let mut s=self.db.conn.prepare("SELECT p.id,p.title,p.current_revision,p.index_state,m.scope FROM pages p JOIN page_meta m ON m.page_id=p.id ORDER BY p.title,p.id").map_err(err)?;
        let pages=s.query_map([],|r|Ok(json!({"id":r.get::<_,String>(0)?,"title":r.get::<_,String>(1)?,"revision":r.get::<_,String>(2)?,"index_state":r.get::<_,String>(3)?,"scope":r.get::<_,String>(4)?}))).map_err(err)?.collect::<std::result::Result<Vec<_>,_>>().map_err(err)?;
        Ok(json!({"sources":sources,"pages":pages}))
    }
    pub fn rebuild_terms(&self) -> Result<()> {
        let mut s=self.db.conn.prepare("SELECT p.id,p.current_revision,p.title,c.text FROM pages p JOIN chunks c ON c.page_id=p.id WHERE p.current_revision=c.revision_id AND p.indexed_revision=c.revision_id AND p.index_state='ready'").map_err(err)?;
        let rows = s
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })
            .map_err(err)?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(err)?;
        drop(s);
        let tx = self.db.conn.unchecked_transaction().map_err(err)?;
        tx.execute("DELETE FROM page_terms", []).map_err(err)?;
        for (p, r, t, b) in rows {
            index_terms(&tx, &p, &r, &t, &b)?;
        }
        tx.commit().map_err(err)
    }
    pub fn search(&self, query: &str) -> Result<Vec<String>> {
        let terms = terms(query);
        if terms.is_empty() {
            return Ok(vec![]);
        }
        let mut scores = std::collections::BTreeMap::<String, i64>::new();
        // Indexed term lookups, never a 10,000-body scan. State/revision guard shares the same read transaction.
        let tx = self.db.conn.unchecked_transaction().map_err(err)?;
        for term in terms {
            let mut s=tx.prepare("SELECT t.page_id,t.weight FROM page_terms t JOIN pages p ON p.id=t.page_id WHERE t.term=? AND p.current_revision=t.revision_id AND p.indexed_revision=t.revision_id AND p.index_state='ready' LIMIT 500").map_err(err)?;
            let rows = s
                .query_map([term], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
                })
                .map_err(err)?;
            for row in rows {
                let (p, n) = row.map_err(err)?;
                *scores.entry(p).or_default() += n;
            }
        }
        tx.commit().map_err(err)?;
        let mut ranked: Vec<_> = scores.into_iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        Ok(ranked.into_iter().take(20).map(|x| x.0).collect())
    }
}
pub fn body_only(text: &str) -> &str {
    text.strip_prefix("---\n")
        .and_then(|s| s.split_once("\n---\n"))
        .map(|(_, body)| body)
        .unwrap_or(text)
}
fn terms(text: &str) -> std::collections::BTreeSet<String> {
    let chars: Vec<_> = text.chars().collect();
    let mut out = std::collections::BTreeSet::new();
    for c in &chars {
        if ('\u{4e00}'..='\u{9fff}').contains(c) {
            out.insert(c.to_string());
        }
    }
    for w in chars.windows(2) {
        if w.iter().all(|c| ('\u{4e00}'..='\u{9fff}').contains(c)) {
            out.insert(w.iter().collect());
        }
    }
    for word in text.split(|c: char| !c.is_alphanumeric()) {
        if !word.is_empty() && word.len() < 80 {
            out.insert(word.to_lowercase());
        }
    }
    out
}
fn index_terms(
    tx: &rusqlite::Transaction<'_>,
    page: &str,
    rev: &str,
    title: &str,
    body: &str,
) -> Result<()> {
    let titleterms = terms(title);
    for term in terms(&format!("{title}\n{body}")) {
        tx.execute("INSERT INTO page_terms VALUES(?1,?2,?3,?4) ON CONFLICT(term,page_id,revision_id) DO UPDATE SET weight=max(weight,excluded.weight)",params![term,page,rev,if titleterms.contains(&term){5}else{1}]).map_err(err)?;
    }
    Ok(())
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PreparedPoint {
    pub id: String,
    pub name: String,
    pub definition: String,
    pub evidence: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PreparedPage {
    pub id: String,
    pub title: String,
    pub scope: String,
    pub definition: String,
    pub body: String,
    pub base: Option<String>,
    pub points: Vec<PreparedPoint>,
    pub links: Vec<String>,
}
impl Library {
    pub fn publish(
        &mut self,
        e: &Execution,
        call: &str,
        sid: &str,
        pages: &[PreparedPage],
        fault: bool,
    ) -> Result<bool> {
        if pages.is_empty() {
            return Err("INVALID_OUTPUT: no pages".into());
        }
        let source = self.source(sid)?;
        let raw = source["body"].as_str().unwrap();
        let mut prepared = vec![];
        let allowed: std::collections::BTreeSet<_> = pages
            .iter()
            .map(|p| p.id.clone())
            .chain({
                let mut s = self.db.conn.prepare("SELECT id FROM pages").map_err(err)?;
                let result = s
                    .query_map([], |r| r.get::<_, String>(0))
                    .map_err(err)?
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map_err(err)?;
                result
            })
            .collect();
        for p in pages {
            uuid::Uuid::parse_str(&p.id).map_err(|_| "INVALID_PAGE_ID")?;
            if p.body.trim().is_empty()
                || p.title.trim().is_empty()
                || p.points.is_empty()
                || p.links.iter().any(|id| !allowed.contains(id))
            {
                return Err("INVALID_OUTPUT: page".into());
            }
            let old = if let Some(base) = &p.base {
                self.page(&p.id, Some(base))?["body"]
                    .as_str()
                    .unwrap()
                    .to_string()
            } else {
                String::new()
            };
            for point in &p.points {
                if point.name.trim().is_empty()
                    || point.definition.trim().is_empty()
                    || point.evidence.trim().is_empty()
                    || !p.body.contains(&point.evidence)
                    || (!raw.contains(&point.evidence) && !old.contains(&point.evidence))
                {
                    return Err("INVALID_OUTPUT: point evidence".into());
                }
            }
            let rev = id();
            let path = format!("wiki/{rev}.md");
            atomic_write(
                &safe_file(&self.root, &path)?,
                format!(
                    "---\npage_id: {}\nrevision_id: {rev}\n---\n{}",
                    p.id, p.body
                )
                .as_bytes(),
            )?;
            prepared.push((p, rev, path));
        }
        let tx = self
            .db
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(err)?;
        if !Store::owns(&tx, e, call)? {
            return Ok(false);
        }
        for (p, _, _) in &prepared {
            let current: Option<String> = tx
                .query_row(
                    "SELECT current_revision FROM pages WHERE id=?",
                    [&p.id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(err)?;
            let pin: Option<Option<String>> = tx
                .query_row(
                    "SELECT revision_id FROM run_page_bases WHERE run_id=? AND page_id=?",
                    params![e.run, p.id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(err)?;
            if current != p.base || pin != Some(p.base.clone()) {
                return Err("CONFLICT: 页面已更新，请重试整理".into());
            }
        }
        for (p, rev, path) in &prepared {
            tx.execute("INSERT INTO pages VALUES(?1,?2,?3,?3,?3,'ready') ON CONFLICT(id) DO UPDATE SET title=excluded.title,current_revision=excluded.current_revision,indexed_revision=excluded.indexed_revision,index_target=excluded.index_target,index_state='ready'",params![p.id,p.title,rev]).map_err(err)?;
            if fault {
                return Err("INJECTED_BEFORE_FTS".into());
            }
            tx.execute(
                "INSERT INTO revisions VALUES(?1,?2,?3,?4,?5)",
                params![rev, p.id, path, hash(&p.body), sid],
            )
            .map_err(err)?;
            tx.execute("DELETE FROM chunk_meta WHERE chunk_rowid IN(SELECT rowid FROM chunks WHERE page_id=?)",[&p.id]).map_err(err)?;
            replace_chunks(&tx, &p.id, rev, &p.title, &p.body)?;
            tx.execute("DELETE FROM page_terms WHERE page_id=?", [&p.id])
                .map_err(err)?;
            index_terms(&tx, &p.id, rev, &p.title, &p.body)?;
            tx.execute("INSERT INTO page_meta VALUES(?1,?2,?3) ON CONFLICT(page_id) DO UPDATE SET scope=excluded.scope,definition=excluded.definition",params![p.id,p.scope,p.definition]).map_err(err)?;
            tx.execute("INSERT OR IGNORE INTO source_pages SELECT source_id,page_id,?1 FROM source_pages WHERE page_id=?2",params![rev,p.id]).map_err(err)?;
            tx.execute(
                "INSERT OR IGNORE INTO source_pages VALUES(?1,?2,?3)",
                params![sid, p.id, rev],
            )
            .map_err(err)?;
            // Retain grounded old point identities when appending new evidence.
            tx.execute(
                "UPDATE points SET revision_id=?1 WHERE page_id=?2",
                params![rev, p.id],
            )
            .map_err(err)?;
            for point in &p.points {
                let existing: Option<String> = tx
                    .query_row("SELECT page_id FROM points WHERE id=?", [&point.id], |r| {
                        r.get(0)
                    })
                    .optional()
                    .map_err(err)?;
                if existing.as_ref().is_some_and(|id| id != &p.id) {
                    return Err("CONFLICT: point identity".into());
                }
                tx.execute("INSERT INTO points VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(id) DO UPDATE SET revision_id=excluded.revision_id,evidence=excluded.evidence",params![point.id,point.name,p.scope,p.id,rev,point.evidence]).map_err(err)?;
                tx.execute(
                    "INSERT INTO point_meta VALUES(?1,?2) ON CONFLICT(point_id) DO NOTHING",
                    params![point.id, point.definition],
                )
                .map_err(err)?;
            }
            tx.execute("INSERT INTO point_versions SELECT p.id,?1,p.name,m.definition,p.evidence FROM points p JOIN point_meta m ON m.point_id=p.id WHERE p.page_id=?2",params![rev,p.id]).map_err(err)?;
            tx.execute(
                "UPDATE index_tasks SET status='superseded' WHERE page_id=?",
                [&p.id],
            )
            .map_err(err)?;
        }
        for (p, rev, _) in &prepared {
            tx.execute("INSERT OR IGNORE INTO page_links SELECT from_page,?1,to_page FROM page_links WHERE from_page=?2",params![rev,p.id]).map_err(err)?;
            for link in &p.links {
                tx.execute(
                    "INSERT OR IGNORE INTO page_links VALUES(?1,?2,?3)",
                    params![p.id, rev, link],
                )
                .map_err(err)?;
            }
        }
        Store::finish(&tx, e)?;
        tx.execute(
            "UPDATE job_meta SET stage='completed',error='',updated_at=? WHERE job_id=?",
            params![now(), e.job],
        )
        .map_err(err)?;
        tx.commit().map_err(err)?;
        Ok(true)
    }
}
