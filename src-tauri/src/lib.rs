pub mod app;
pub mod budget;
pub mod chat;
pub mod chat_run;
pub mod config;
pub mod explanation;
pub mod ingest;
pub mod knowledge;
pub mod learning;
pub mod mastery;
pub mod backup;
pub mod model;
pub mod retrieval;
pub mod store;
pub mod workflow;

use serde_json::{json, Value};
use std::path::Path;
pub type Result<T> = std::result::Result<T, String>;
pub fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}
pub fn hash(text: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(text.as_bytes()))
}
pub fn storage_probe(root: &Path) -> Result<Value> {
    std::fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let mut db = store::Store::open(root)?;
    let source_id = db.save_source("M0原始资料", "# 催化剂\n降低活化能；不改变平衡常数。")?;
    db.conn
        .execute(
            "INSERT INTO probe_counter(id,n) VALUES(1,1) ON CONFLICT(id) DO UPDATE SET n=n+1",
            [],
        )
        .map_err(|e| e.to_string())?;
    let n: i64 = db
        .conn
        .query_row("SELECT n FROM probe_counter WHERE id=1", [], |r| r.get(0))
        .map_err(|e| e.to_string())?;
    let config_path = root.join("settings.json");
    let settings = config::Settings::load(&config_path)?;
    settings.save(&config_path)?;
    Ok(
        json!({"ipc":"Rust command executed", "scope":"M0 isolated sandbox", "root":root,
        "sqlite_version":rusqlite::version(), "schema_version":db.version()?, "source_id":source_id,
        "markdown_exists":root.join("sources").join(format!("{source_id}.md")).exists(),
        "persisted_invocations":n,"config_restored":config::Settings::load(&config_path)?.config_revision==settings.config_revision}),
    )
}
