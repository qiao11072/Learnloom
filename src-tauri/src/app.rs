//! Application coordinator: persistent setup, grants, a single library writer, and jobs.
use crate::{
    config::{self, Settings, Snapshot},
    id,
    knowledge::{self, atomic_write, now, Library},
    model,
    store::Execution,
    Result,
};
use fs2::FileExt;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use tokio_util::sync::CancellationToken;
#[derive(Clone, Serialize, Deserialize)]
pub struct LibraryRef {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Setup {
    pub step: u8,
    pub deferred: bool,
    pub completed: bool,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub schema_version: u32,
    pub model: Settings,
    pub setup: Setup,
    pub active_library: Option<LibraryRef>,
    pub sequence: u64,
}
impl Default for AppSettings {
    fn default() -> Self {
        Self {
            schema_version: 1,
            model: Settings::default(),
            setup: Setup {
                step: 0,
                deferred: false,
                completed: false,
            },
            active_library: None,
            sequence: 0,
        }
    }
}
struct Grant {
    path: PathBuf,
    file: bool,
}
pub(crate) struct Inner {
    pub settings: AppSettings,
    pub library: Option<Library>,
    pub warning: String,
    pub recovered: usize,
    allow_mock: bool,
    grants: BTreeMap<String, Grant>,
    pub tokens: BTreeMap<String, CancellationToken>,
    pub spawned: BTreeSet<String>,
    _profile_lock: File,
}
pub struct AppCore {
    pub profile: PathBuf,
    pub(crate) inner: Mutex<Inner>,
    pub(crate) serial: Arc<tokio::sync::Semaphore>,
    pub(crate) updates: tokio::sync::broadcast::Sender<crate::chat::Update>,
    pub(crate) subscriptions: Mutex<BTreeMap<String, CancellationToken>>,
}
#[derive(Debug, Serialize)]
pub struct AppError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub request_id: String,
}
impl From<String> for AppError {
    fn from(message: String) -> Self {
        let head = message.split(':').next().unwrap_or("STORAGE_ERROR");
        let code = if head.chars().all(|c| c.is_ascii_uppercase() || c == '_') {
            head.to_owned()
        } else {
            "STORAGE_ERROR".into()
        };
        let retryable = [
            "TIMEOUT",
            "NETWORK_ERROR",
            "RATE_LIMITED",
            "PROVIDER_ERROR",
            "CONFLICT",
            "MODEL_NOT_READY",
        ]
        .contains(&code.as_str());
        Self {
            code,
            message,
            retryable,
            request_id: id(),
        }
    }
}
fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}
fn arg<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    v[key]
        .as_str()
        .ok_or_else(|| format!("INVALID_ARGUMENT: {key}"))
}
fn expected(v: &Value, key: &str) -> Result<u64> {
    v[key]
        .as_u64()
        .ok_or_else(|| format!("INVALID_ARGUMENT: {key}"))
}
impl AppCore {
    pub fn new(profile: &Path) -> Result<Arc<Self>> {
        Self::new_impl(profile, false)
    }
    #[cfg(feature = "m0-tools")]
    pub fn for_test(profile: &Path) -> Result<Arc<Self>> {
        Self::new_impl(profile, true)
    }
    fn new_impl(profile: &Path, allow_mock: bool) -> Result<Arc<Self>> {
        fs::create_dir_all(profile).map_err(err)?;
        knowledge::no_reparse(profile)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(profile.join("app.lock"))
            .map_err(err)?;
        lock.try_lock_exclusive()
            .map_err(|_| "APP_ALREADY_RUNNING")?;
        let path = profile.join("settings.json");
        let mut settings: AppSettings = if path.exists() {
            serde_json::from_slice(&fs::read(&path).map_err(err)?)
                .map_err(|_| "SETTINGS_CORRUPTED: 设置文件损坏，请保留文件并恢复备份")?
        } else {
            AppSettings::default()
        };
        if settings.schema_version != 1 {
            return Err("UNSUPPORTED_SETTINGS_VERSION".into());
        }
        if settings.model.check_state == "testing" {
            settings.model.check_state = "untested".into();
            settings.model.active_check = None;
        }
        let mut library = None;
        let mut warning = String::new();
        let mut recovered = 0;
        if let Some(active) = &settings.active_library {
            match Library::open(&active.path, "", false) {
                Ok(mut lib) => {
                    if lib.marker.id != active.id {
                        return Err("KB_ID_MISMATCH".into());
                    }
                    match lib.recover() {
                        Ok(n) => {
                            recovered = n;
                            library = Some(lib)
                        }
                        Err(e) => warning = e,
                    }
                }
                Err(e) => warning = e,
            }
        }
        fs::create_dir_all(profile.join("knowledge-bases")).map_err(err)?;
        let default_path = settings
            .active_library
            .as_ref()
            .map(|l| l.path.clone())
            .unwrap_or_else(|| profile.join("knowledge-bases/我的知识库"));
        let mut grants = BTreeMap::new();
        grants.insert(
            "default".into(),
            Grant {
                path: default_path,
                file: false,
            },
        );
        let app = Arc::new(Self {
            profile: profile.into(),
            inner: Mutex::new(Inner {
                settings,
                library,
                warning,
                recovered,
                allow_mock,
                grants,
                tokens: BTreeMap::new(),
                spawned: BTreeSet::new(),
                _profile_lock: lock,
            }),
            serial: Arc::new(tokio::sync::Semaphore::new(1)),
            updates: tokio::sync::broadcast::channel(256).0,
            subscriptions: Mutex::new(BTreeMap::new()),
        });
        app.persist()?;
        Ok(app)
    }
    fn commit_locked(&self, inner: &mut Inner, mut next: AppSettings) -> Result<()> {
        next.sequence = inner.settings.sequence + 1;
        atomic_write(
            &self.profile.join("settings.json"),
            &serde_json::to_vec_pretty(&next).map_err(err)?,
        )?;
        inner.settings = next;
        Ok(())
    }
    fn save_locked(&self, inner: &mut Inner) -> Result<()> {
        self.commit_locked(inner, inner.settings.clone())
    }
    fn persist(&self) -> Result<()> {
        let mut i = self.inner.lock().map_err(|_| "STATE_LOCK")?;
        self.save_locked(&mut i)
    }
    pub fn grant_directory(&self, path: &Path) -> Result<Value> {
        let path = knowledge::normalize_location(path)?;
        let token = id();
        self.inner.lock().map_err(|_| "STATE_LOCK")?.grants.insert(
            token.clone(),
            Grant {
                path: path.clone(),
                file: false,
            },
        );
        Ok(json!({"token":token,"path":path}))
    }
    pub fn grant_file(&self, path: &Path) -> Result<Value> {
        let path = fs::canonicalize(path).map_err(err)?;
        if !path.is_file() {
            return Err("NOT_A_FILE".into());
        }
        let token = id();
        self.inner.lock().map_err(|_| "STATE_LOCK")?.grants.insert(
            token.clone(),
            Grant {
                path: path.clone(),
                file: true,
            },
        );
        Ok(json!({"token":token,"name":path.file_name().unwrap_or_default().to_string_lossy()}))
    }
    pub fn backup_library(&self,token:&str)->Result<Value>{
        let i=self.inner.lock().map_err(|_|"STATE_LOCK")?;
        let grant=i.grants.get(token).ok_or("DIRECTORY_NOT_AUTHORIZED")?;if grant.file{return Err("DIRECTORY_NOT_AUTHORIZED".into())}
        crate::backup::create(i.library.as_ref().ok_or("NO_LIBRARY")?,&grant.path)
    }
    pub fn restore_library_backup(&self,source:&str,target:&str)->Result<Value>{
        let i=self.inner.lock().map_err(|_|"STATE_LOCK")?;
        let src=i.grants.get(source).ok_or("DIRECTORY_NOT_AUTHORIZED")?;let dst=i.grants.get(target).ok_or("DIRECTORY_NOT_AUTHORIZED")?;
        if src.file||dst.file{return Err("DIRECTORY_NOT_AUTHORIZED".into())}
        if let Some(lib)=&i.library{if dst.path.starts_with(&lib.root){return Err("UNSAFE_BACKUP_TARGET".into())}}
        crate::backup::restore(&src.path,&dst.path)
    }
    pub fn inspect_location(&self, token: &str) -> Result<Value> {
        let i = self.inner.lock().map_err(|_| "STATE_LOCK")?;
        let g = i.grants.get(token).ok_or("DIRECTORY_NOT_AUTHORIZED")?;
        if g.file {
            return Err("DIRECTORY_NOT_AUTHORIZED".into());
        }
        knowledge::inspect(&g.path)
    }
    pub fn open_library(&self, token: &str, name: &str, create: bool) -> Result<Value> {
        let mut i = self.inner.lock().map_err(|_| "STATE_LOCK")?;
        if let Some(lib) = &i.library {
            let busy: bool = lib
                .db
                .conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM jobs WHERE status IN('queued','running'))",
                    [],
                    |r| r.get(0),
                )
                .map_err(err)?;
            if busy {
                return Err("BUSY: 请先完成或取消当前后台任务".into());
            }
        }
        let path = {
            let g = i.grants.get(token).ok_or("DIRECTORY_NOT_AUTHORIZED")?;
            if g.file {
                return Err("DIRECTORY_NOT_AUTHORIZED".into());
            }
            knowledge::normalize_location(&g.path)?
        };
        if i.library.as_ref().is_some_and(|l| l.root == path) {
            return Ok(json!({"id":i.library.as_ref().unwrap().marker.id,"already_open":true}));
        }
        let mut lib = Library::open(&path, name, create)?;
        let recovered = lib.recover()?;
        let active = LibraryRef {
            id: lib.marker.id.clone(),
            name: lib.marker.name.clone(),
            path: lib.root.clone(),
        };
        let mut next = i.settings.clone();
        next.active_library = Some(active.clone());
        next.setup.step = next.setup.step.max(1);
        self.commit_locked(&mut i, next)?;
        i.grants.insert(
            "default".into(),
            Grant {
                path: active.path,
                file: false,
            },
        );
        i.library = Some(lib);
        i.warning.clear();
        i.recovered = recovered;
        Ok(json!({"opened":true}))
    }
    pub fn bootstrap(&self) -> Result<Value> {
        let i = self.inner.lock().map_err(|_| "STATE_LOCK")?;
        let m = &i.settings.model;
        let has_key = m
            .current
            .credential_ref
            .as_ref()
            .map(|r| config::read_credential(r).is_ok())
            .unwrap_or(false);
        let lists = if let Some(lib) = &i.library {
            lib.lists()?
        } else {
            json!({"sources":[],"pages":[]})
        };
        Ok(
            json!({"setup":i.settings.setup,"sequence":i.settings.sequence,"settings":{"config_revision":m.config_revision,"preferences_revision":m.current.preferences_revision,"endpoint":m.current.endpoint,"model":m.current.model,"context":m.current.context,"language":m.current.language,"has_key":has_key,"check_state":m.check_state,"model_ready":m.check_state=="passed"&& (m.current.credential_ref.is_none()||has_key)},"library":i.library.as_ref().map(|l|json!({"id":l.marker.id,"name":l.marker.name,"path":l.root})),"default_location":{"token":"default","path":i.grants["default"].path},"sources":lists["sources"],"pages":lists["pages"],"warning":i.warning,"unfinished_review_sessions":i.library.as_ref().map(crate::learning::list).transpose()?.map(|v|v["items"].as_array().unwrap().iter().filter(|s|s["status"]!="completed").cloned().collect::<Vec<_>>()).unwrap_or_default(),"recovered_jobs":i.recovered}),
        )
    }
    pub fn save_model(&self, v: &Value) -> Result<Value> {
        let endpoint = config::validate_cloud(arg(v, "endpoint")?, arg(v, "model")?)?;
        let model = arg(v, "model")?.trim();
        let key = arg(v, "key")?;
        let context = v["context"].as_u64().ok_or("INVALID_CONTEXT_LIMIT")? as usize;
        if !(2048..=1_048_576).contains(&context) {
            return Err("INVALID_CONTEXT_LIMIT".into());
        }
        let mut i = self.inner.lock().map_err(|_| "STATE_LOCK")?;
        if expected(v, "expected_config_revision")? != i.settings.model.config_revision {
            return Err("CONFLICT: 模型设置已变，请重新加载".into());
        }
        let old = &i.settings.model.current;
        let reference = if !key.is_empty() {
            let reference = format!("learnloom-{}", id());
            config::write_credential(&reference, key)?;
            Some(reference)
        } else if endpoint == old.endpoint {
            old.credential_ref.clone()
        } else {
            None
        };
        if reqwest::Url::parse(&endpoint)
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned))
            .as_deref()
            == Some("api.deepseek.com")
            && reference.is_none()
        {
            return Err("CREDENTIAL_REQUIRED: 请填写API Key".into());
        }
        let mut next = i.settings.clone();
        next.model.replace(&endpoint, model, reference);
        next.model.current.context = context;
        let rev = next.model.config_revision;
        let snap = next.model.current.clone();
        next.model.versions.insert(rev, snap);
        next.setup.step = next.setup.step.max(2);
        self.commit_locked(&mut i, next)?;
        drop(i);
        self.bootstrap()
    }
    pub fn save_language(&self, v: &Value) -> Result<Value> {
        let language = arg(v, "language")?;
        if !["zh-CN", "en"].contains(&language) {
            return Err("INVALID_LANGUAGE".into());
        }
        let mut i = self.inner.lock().map_err(|_| "STATE_LOCK")?;
        if expected(v, "expected_preferences_revision")?
            != i.settings.model.current.preferences_revision
        {
            return Err("CONFLICT: 偏好已变".into());
        }
        let mut next = i.settings.clone();
        next.model.language(language);
        self.commit_locked(&mut i, next)?;
        drop(i);
        self.bootstrap()
    }
    pub fn setup_action(&self, action: &str) -> Result<Value> {
        let mut i = self.inner.lock().map_err(|_| "STATE_LOCK")?;
        let mut next = i.settings.clone();
        if action == "complete" {
            if i.library.is_none() || i.settings.model.check_state != "passed" {
                return Err("SETUP_NOT_READY: 请完成目录和模型连接检查".into());
            }
            next.setup.completed = true;
            next.setup.deferred = false;
            next.setup.step = 3;
        } else if action == "defer" {
            next.setup.deferred = true;
        } else if action == "continue" {
            next.setup.deferred = false;
        } else {
            return Err("INVALID_ACTION".into());
        }
        self.commit_locked(&mut i, next)?;
        drop(i);
        self.bootstrap()
    }
    pub async fn check_connection(self: &Arc<Self>, revision: u64) -> Result<Value> {
        let (snapshot, check, token) = {
            let mut i = self.inner.lock().map_err(|_| "STATE_LOCK")?;
            if revision != i.settings.model.config_revision {
                return Err("CONFLICT: 模型设置已变".into());
            }
            if !i.allow_mock {
                config::validate_cloud(
                    &i.settings.model.current.endpoint,
                    &i.settings.model.current.model,
                )?;
            }
            let mut next = i.settings.clone();
            let (_, check) = next.model.begin_check();
            let token = CancellationToken::new();
            let snap = next.model.current.clone();
            self.commit_locked(&mut i, next)?;
            i.tokens.insert(check.clone(), token.clone());
            (snap, check, token)
        };
        // A connectivity probe does not need the full model context. Keep its
        // output/reservation small without changing the saved model capacity.
        let mut probe = snapshot.clone();
        probe.context = probe.context.min(4096);
        let result = model::generate(
            &probe,
            "Return JSON only {ok:true}.",
            &json!({"test":"connection"}),
            std::time::Duration::from_secs(15),
            token,
        )
        .await;
        let passed = result
            .as_ref()
            .ok()
            .and_then(|r| serde_json::from_str::<Value>(&r.content).ok())
            .map(|v| v["ok"] == true)
            .unwrap_or(false);
        let mut i = self.inner.lock().map_err(|_| "STATE_LOCK")?;
        let mut next = i.settings.clone();
        let applied = next.model.finish_check(revision, &check, passed);
        self.commit_locked(&mut i, next)?;
        i.tokens.remove(&check);
        drop(i);
        Ok(
            json!({"passed":passed,"applied":applied,"error":result.err(),"bootstrap":self.bootstrap()?}),
        )
    }
    pub fn cancel_check(&self) -> Result<Value> {
        let mut i = self.inner.lock().map_err(|_| "STATE_LOCK")?;
        if let Some(check) = i.settings.model.active_check.clone() {
            let mut next = i.settings.clone();
            next.model.cancel_check(&check);
            self.commit_locked(&mut i, next)?;
            if let Some(token) = i.tokens.get(&check) {
                token.cancel();
            }
        }
        drop(i);
        self.bootstrap()
    }
    pub(crate) fn model_snapshot(i: &Inner) -> Result<Snapshot> {
        if i.settings.model.check_state != "passed" {
            return Err("MODEL_NOT_READY: 请先保存模型并通过连接测试".into());
        }
        let snapshot = i.settings.model.current.clone();
        if !i.allow_mock {
            config::validate_cloud(&snapshot.endpoint, &snapshot.model)?;
        }
        if let Some(r) = &snapshot.credential_ref {
            config::read_credential(r)?;
        }
        Ok(snapshot)
    }
    pub fn save_source(&self, v: &Value) -> Result<Value> {
        let mut i = self.inner.lock().map_err(|_| "STATE_LOCK")?;
        let lib = i.library.as_mut().ok_or("NO_LIBRARY")?;
        lib.save_source(
            arg(v, "title")?,
            arg(v, "text")?,
            "text",
            arg(v, "request_key")?,
        )
    }
    pub fn import_file(&self, v: &Value) -> Result<Value> {
        let mut i = self.inner.lock().map_err(|_| "STATE_LOCK")?;
        let path = {
            let g = i
                .grants
                .get(arg(v, "token")?)
                .ok_or("FILE_NOT_AUTHORIZED")?;
            if !g.file {
                return Err("FILE_NOT_AUTHORIZED".into());
            }
            g.path.clone()
        };
        let ext = path
            .extension()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase();
        if !["txt", "md", "markdown"].contains(&ext.as_str()) {
            return Err("UNSUPPORTED_FILE_TYPE: 仅支持文本和UTF-8 Markdown".into());
        }
        if fs::metadata(&path).map_err(err)?.len() > 16 * 1024 * 1024 {
            return Err("SOURCE_TOO_LARGE".into());
        }
        let bytes = fs::read(&path).map_err(err)?;
        let text = String::from_utf8(bytes).map_err(|_| "INVALID_UTF8: 请将文件另存为UTF-8")?;
        let text = text.trim_start_matches('\u{feff}');
        let title = path.file_stem().unwrap_or_default().to_string_lossy();
        i.library.as_mut().ok_or("NO_LIBRARY")?.save_source(
            &title,
            text,
            &ext,
            arg(v, "request_key")?,
        )
    }
    pub fn start_ingest(self: &Arc<Self>, sid: &str) -> Result<Value> {
        let (e, kbid) = {
            let mut i = self.inner.lock().map_err(|_| "STATE_LOCK")?;
            let snapshot = Self::model_snapshot(&i)?;
            let lib = i.library.as_mut().ok_or("NO_LIBRARY")?;
            let source = lib.source(sid)?;
            let key = format!("ingest:{sid}:{}:m1-v1", source["hash"].as_str().unwrap());
            let e = lib.db.enqueue(
                &key,
                "organize",
                sid,
                source["hash"].as_str().unwrap(),
                &snapshot,
            )?;
            lib.db
                .conn
                .execute(
                    "INSERT OR IGNORE INTO job_meta VALUES(?1,'queued','',?2,?2)",
                    params![e.job, now()],
                )
                .map_err(err)?;
            (e, lib.marker.id.clone())
        };
        self.schedule(kbid, e.clone());
        Ok(json!({"job_id":e.job,"run_id":e.run}))
    }
    pub fn retry_job(self: &Arc<Self>, v: &Value) -> Result<Value> {
        let (e, kbid) = {
            let mut i = self.inner.lock().map_err(|_| "STATE_LOCK")?;
            let snapshot = Self::model_snapshot(&i)?;
            let lib = i.library.as_mut().ok_or("NO_LIBRARY")?;
            let old = Execution {
                job: arg(v, "job_id")?.into(),
                run: arg(v, "expected_run_id")?.into(),
            };
            let e = lib.db.retry(&old, arg(v, "request_key")?, &snapshot)?;
            lib.db
                .conn
                .execute(
                    "UPDATE job_meta SET stage='queued',error='',updated_at=? WHERE job_id=?",
                    params![now(), e.job],
                )
                .map_err(err)?;
            if let Some(event) = crate::chat::job_update(lib, &e.job, "reset")? {
                self.emit(event);
            }
            (e, lib.marker.id.clone())
        };
        self.schedule(kbid, e.clone());
        Ok(json!({"job_id":e.job,"run_id":e.run}))
    }
    pub fn cancel_job(&self, v: &Value) -> Result<Value> {
        let mut i = self.inner.lock().map_err(|_| "STATE_LOCK")?;
        let e = Execution {
            job: arg(v, "job_id")?.into(),
            run: arg(v, "expected_run_id")?.into(),
        };
        let lib = i.library.as_mut().ok_or("NO_LIBRARY")?;
        let active: String = lib
            .db
            .conn
            .query_row("SELECT active_run FROM jobs WHERE id=?", [&e.job], |r| {
                r.get(0)
            })
            .map_err(err)?;
        if active != e.run {
            return Err("CONFLICT: 任务已进入新代次".into());
        }
        let stopped = lib.db.stop(&e, "cancelled")?;
        if stopped {
            lib.db.conn.execute("UPDATE job_meta SET stage='cancelled',error='已取消，可显式重试',updated_at=? WHERE job_id=?",params![now(),e.job]).map_err(err)?;
            if let Some(event) = crate::chat::job_update(lib, &e.job, "replace")? {
                self.emit(event);
            }
            if let Some(token) = i.tokens.get(&e.run) {
                token.cancel();
            }
        }
        Ok(json!({"cancelled":stopped,"already_finished":!stopped}))
    }
    pub fn source(&self, sid: &str) -> Result<Value> {
        self.inner
            .lock()
            .map_err(|_| "STATE_LOCK")?
            .library
            .as_ref()
            .ok_or("NO_LIBRARY")?
            .source(sid)
    }
    pub fn page(&self, pid: &str, revision: Option<&str>) -> Result<Value> {
        self.inner
            .lock()
            .map_err(|_| "STATE_LOCK")?
            .library
            .as_ref()
            .ok_or("NO_LIBRARY")?
            .page(pid, revision)
    }
    pub fn search(&self, query: &str) -> Result<Value> {
        if query.chars().count() > 200 {
            return Err("QUERY_TOO_LONG".into());
        }
        let i = self.inner.lock().map_err(|_| "STATE_LOCK")?;
        let lib = i.library.as_ref().ok_or("NO_LIBRARY")?;
        serde_json::to_value(crate::retrieval::search(lib, query, 20)?).map_err(err)
    }
    pub fn job(&self, jid: &str) -> Result<Value> {
        let i = self.inner.lock().map_err(|_| "STATE_LOCK")?;
        let lib = i.library.as_ref().ok_or("NO_LIBRARY")?;
        let (status, run, seq): (String, String, i64) = lib
            .db
            .conn
            .query_row(
                "SELECT status,active_run,seq FROM jobs WHERE id=?",
                [jid],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map_err(err)?;
        let snapshot = lib.db.snapshot(&run)?;
        let calls: i64 = lib
            .db
            .conn
            .query_row(
                "SELECT count(*) FROM model_calls WHERE run_id=?",
                [&run],
                |r| r.get(0),
            )
            .map_err(err)?;
        let (stage, error): (String, String) = lib
            .db
            .conn
            .query_row(
                "SELECT stage,error FROM job_meta WHERE job_id=?",
                [jid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(err)?;
        Ok(
            json!({"id":jid,"status":status,"run_id":run,"sequence":seq,"config_revision":snapshot.config_revision,"preferences_revision":snapshot.preferences_revision,"language":snapshot.language,"model":snapshot.model,"calls":calls,"stage":stage,"error":error}),
        )
    }
    pub fn resume_queued(self: &Arc<Self>) -> Result<()> {
        let queued = {
            let i = self.inner.lock().map_err(|_| "STATE_LOCK")?;
            if let Some(lib) = &i.library {
                let mut s = lib
                    .db
                    .conn
                    .prepare(
                        "SELECT id,active_run FROM jobs WHERE kind='organize' AND status='queued'",
                    )
                    .map_err(err)?;
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
                Some((lib.marker.id.clone(), rows))
            } else {
                None
            }
        };
        if let Some((kb, rows)) = queued {
            for e in rows {
                self.schedule(kb.clone(), e);
            }
        }
        Ok(())
    }
    pub(crate) fn with_library<T>(
        &self,
        kb: &str,
        f: impl FnOnce(&mut Library) -> Result<T>,
    ) -> Result<T> {
        let mut i = self.inner.lock().map_err(|_| "STATE_LOCK")?;
        let lib = i.library.as_mut().ok_or("NO_LIBRARY")?;
        if lib.marker.id != kb {
            return Err("LIBRARY_CHANGED".into());
        }
        f(lib)
    }
    pub(crate) async fn blocking<T: Send + 'static>(
        self: &Arc<Self>,
        kb: String,
        f: impl FnOnce(&mut Library) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let app = self.clone();
        tokio::task::spawn_blocking(move || app.with_library(&kb, f))
            .await
            .map_err(err)?
    }
    pub(crate) fn schedule(self: &Arc<Self>, kb: String, e: Execution) {
        let (token, kind) = {
            let mut i = match self.inner.lock() {
                Ok(i) => i,
                Err(_) => return,
            };
            if !i.spawned.insert(e.run.clone()) {
                return;
            }
            let t = CancellationToken::new();
            i.tokens.insert(e.run.clone(), t.clone());
            let kind = i
                .library
                .as_ref()
                .and_then(|lib| {
                    lib.db
                        .conn
                        .query_row("SELECT kind FROM jobs WHERE id=?", [&e.job], |r| {
                            r.get::<_, String>(0)
                        })
                        .ok()
                })
                .unwrap_or_default();
            (t, kind)
        };
        let app = self.clone();
        tokio::spawn(async move {
            let permit = tokio::select! {biased;_=token.cancelled()=>None,p=app.serial.clone().acquire_owned()=>p.ok()};
            if let Some(_permit) = permit {
                let run = async {
                    if kind == "review" || kind == "grade" {
                        crate::learning::run(app.clone(), kb.clone(), e.clone(), token.clone())
                            .await
                    } else if kind == "chat" {
                        crate::chat_run::run(app.clone(), kb.clone(), e.clone(), token.clone())
                            .await
                    } else {
                        crate::ingest::run(app.clone(), kb.clone(), e.clone(), token.clone()).await
                    }
                };
                let outcome = tokio::time::timeout(
                    std::time::Duration::from_secs(
                        if kind == "chat" || kind == "grade" || kind == "review" {
                            300
                        } else {
                            1800
                        },
                    ),
                    run,
                )
                .await
                .unwrap_or_else(|_| Err("TIMEOUT: 任务达到总时限".into()));
                if let Err(error) = outcome {
                    let e2 = e.clone();
                    let event=app.blocking(kb.clone(),move|lib|{let changed=lib.db.stop(&e2,if error.starts_with("CANCELLED"){"cancelled"}else{"failed"})?;if changed{lib.db.conn.execute("UPDATE job_meta SET stage='failed',error=?,updated_at=? WHERE job_id=?",params![error,now(),e2.job]).map_err(err)?;crate::chat::job_update(lib,&e2.job,"replace")}else{Ok(None)}}).await;
                    if let Ok(Some(event)) = event {
                        app.emit(event);
                    }
                }
            }
            if let Ok(mut i) = app.inner.lock() {
                i.spawned.remove(&e.run);
                i.tokens.remove(&e.run);
            }
        });
    }
    pub async fn dispatch(self: &Arc<Self>, operation: &str, v: Value) -> Result<Value> {
        if operation == "test_model_connection" {
            return self
                .check_connection(expected(&v, "config_revision")?)
                .await;
        }
        let app = self.clone();
        let op = operation.to_string();
        tokio::task::spawn_blocking(move || match op.as_str() {
            "get_bootstrap_state" => app.bootstrap(),
            "inspect_kb_location" => app.inspect_location(arg(&v, "token")?),
            "create_or_open_kb" => app.open_library(
                arg(&v, "token")?,
                v["name"].as_str().unwrap_or(""),
                v["create"].as_bool().ok_or("INVALID_ARGUMENT")?,
            ),
            "save_model_config" => app.save_model(&v),
            "save_preferences" => app.save_language(&v),
            "setup_action" => app.setup_action(arg(&v, "action")?),
            "cancel_connection_check" => app.cancel_check(),
            "save_source" => app.save_source(&v),
            "import_source_file" => app.import_file(&v),
            "get_source" => app.source(arg(&v, "source_id")?),
            "backup_library" => app.backup_library(arg(&v,"token")?),
            "restore_library_backup" => app.restore_library_backup(arg(&v,"source_token")?,arg(&v,"target_token")?),
            "get_mastery" => app.get_mastery(v["filter"].as_str().unwrap_or("all")),
            "get_point_history" => app.point_history(arg(&v,"point_id")?),
            "rebuild_mastery" => app.rebuild_mastery(),
            "get_daily_session" => app.daily(),
            "list_review_sessions" => app.review_list(),
            "get_review_session" => app.review_session(arg(&v, "session_id")?),
            "save_answer_draft" => app.save_draft(&v),
            "submit_answer" => app.submit_review(&v),
            "ask_knowledge" => app.ask(&v),
            "list_conversations" => app.conversations(
                v["cursor"].as_str(),
                v["limit"].as_u64().unwrap_or(30) as usize,
            ),
            "get_conversation" => app.conversation(
                arg(&v, "conversation_id")?,
                v["message_cursor"].as_i64(),
                v["limit"].as_u64().unwrap_or(30) as usize,
            ),
            "get_message" => {
                serde_json::to_value(app.message(arg(&v, "message_id")?)?).map_err(err)
            }
            "get_page" => app.page(arg(&v, "page_id")?, v["revision"].as_str()),
            "search_knowledge" => app.search(arg(&v, "query")?),
            "start_ingest" => app.start_ingest(arg(&v, "source_id")?),
            "retry_job" => app.retry_job(&v),
            "get_job" => app.job(arg(&v, "job_id")?),
            "cancel_job" => app.cancel_job(&v),
            _ => Err("UNKNOWN_COMMAND".into()),
        })
        .await
        .map_err(err)?
    }
    #[cfg(feature = "m0-tools")]
    pub fn seed_test_model(&self, snapshot: Snapshot, passed: bool) -> Result<()> {
        let mut i = self.inner.lock().map_err(|_| "STATE_LOCK")?;
        i.settings.model.current = snapshot.clone();
        i.settings.model.config_revision = snapshot.config_revision;
        i.settings
            .model
            .versions
            .insert(snapshot.config_revision, snapshot);
        i.settings.model.check_state = if passed { "passed" } else { "untested" }.into();
        self.save_locked(&mut i)
    }
}
