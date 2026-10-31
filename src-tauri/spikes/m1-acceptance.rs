// Explicit paid acceptance helper; not built into the desktop application.
use learnloom_m0::{app::AppCore, id, Result};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
async fn wait(app: &Arc<AppCore>, job: &str) -> Result<Value> {
    for _ in 0..3600 {
        let v = app.job(job)?;
        if !["queued", "running"].contains(&v["status"].as_str().unwrap_or("")) {
            return Ok(v);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Err("ACCEPTANCE_TIMEOUT".into())
}
#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let action = args.get(1).map(String::as_str).unwrap_or("short");
    let profile = PathBuf::from(args.get(2).ok_or("profile path required")?);
    let app = AppCore::new(&profile)?;
    if app.bootstrap()?["library"].is_null() {
        app.open_library("default", "M1 真实模型 验收", true)?;
    }
    if matches!(action, "seed" | "seed-custom")
        || !app.bootstrap()?["settings"]["has_key"]
            .as_bool()
            .unwrap_or(false)
    {
        let state = app.bootstrap()?;
        let key = std::env::var("dsh_key").map_err(|_| "dsh_key not available")?;
        app.save_model(&json!({"endpoint":if action=="seed-custom"{"https://api.deepseek.com/v1"}else{"https://api.deepseek.com"},"model":"deepseek-flash","key":key,"context":4096,"expected_config_revision":state["settings"]["config_revision"]}))?;
        let rev = app.bootstrap()?["settings"]["config_revision"]
            .as_u64()
            .unwrap();
        let check = app.check_connection(rev).await?;
        if check["passed"] != true {
            return Err(format!("connection test failed: {}", check["error"]));
        }
    }
    if matches!(action, "seed" | "seed-custom") {
        println!(
            "Cloud configuration saved to system credential store; connection passed. Profile: {}",
            profile.display()
        );
        return Ok(());
    }
    let unit="# 催化剂与平衡\n催化剂提供另一条反应路径，降低活化能，加快正逆反应达到平衡的速度。在温度不变时，催化剂不改变平衡常数，也不改变平衡位置。\n# 密度\n密度定义为单位体积的质量，关系式为ρ=m/V。质量和体积须使用一致的单位。固定温度下，同种均匀物质的密度不随取样大小改变。\n";
    let (title, text) = if action.starts_with("long") {
        (
            "催化剂与密度长资料",
            if action == "long-final" {
                format!("{}\n", unit.repeat(110))
            } else {
                unit.repeat(110)
            },
        )
    } else {
        ("催化剂与密度", unit.to_string())
    };
    let source = app.save_source(
        &json!({"title":title,"text":text,"request_key":format!("m1-real-{action}-{}",id())}),
    )?;
    let mut execution = app.start_ingest(source["source_id"].as_str().unwrap())?;
    if ["failed", "interrupted", "cancelled"].contains(
        &app.job(execution["job_id"].as_str().unwrap())?["status"]
            .as_str()
            .unwrap_or(""),
    ) {
        execution=app.retry_job(&json!({"job_id":execution["job_id"],"expected_run_id":execution["run_id"],"request_key":id()}))?;
    }
    let job = wait(&app, execution["job_id"].as_str().unwrap()).await?;
    let bootstrap = app.bootstrap()?;
    let mut pages = vec![];
    for p in bootstrap["pages"].as_array().unwrap() {
        pages.push(app.page(p["id"].as_str().unwrap(), None)?);
    }
    let root = Path::new(bootstrap["library"]["path"].as_str().unwrap());
    let db = rusqlite::Connection::open(root.join(".learnloom/learnloom.sqlite3"))
        .map_err(|e| e.to_string())?;
    let mut s=db.prepare("SELECT stage,status,config_revision,preferences_revision,prompt_hash,budget,usage,error FROM model_calls WHERE run_id=? ORDER BY rowid").map_err(|e|e.to_string())?;
    let calls=s.query_map([execution["run_id"].as_str().unwrap()],|r|Ok(json!({"stage":r.get::<_,String>(0)?,"status":r.get::<_,String>(1)?,"config_revision":r.get::<_,i64>(2)?,"preferences_revision":r.get::<_,i64>(3)?,"prompt_hash":r.get::<_,String>(4)?,"budget":r.get::<_,String>(5)?,"usage":r.get::<_,String>(6)?,"error":r.get::<_,Option<String>>(7)?}))).map_err(|e|e.to_string())?.collect::<std::result::Result<Vec<_>,_>>().map_err(|e|e.to_string())?;
    let chunks: i64 = db
        .query_row(
            "SELECT count(*) FROM source_chunks WHERE source_id=?",
            [source["source_id"].as_str().unwrap()],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    let out = json!({"kind":"real-cloud-production-pipeline","action":action,"source_bytes":text.len(),"context":4096,"source":app.source(source["source_id"].as_str().unwrap())?,"job":job,"pages":pages,"calls":calls,"chunks":chunks,"search":app.search("平衡位置")?});
    std::fs::create_dir_all("docs/m1/artifacts").map_err(|e| e.to_string())?;
    let path = format!("docs/m1/artifacts/real-{action}-{}.json", id());
    std::fs::write(&path, serde_json::to_vec_pretty(&out).unwrap()).map_err(|e| e.to_string())?;
    println!(
        "Result {}: {}; {} calls, {} chunks, {} bytes. Evidence: {path}",
        job["status"],
        job["error"],
        calls.len(),
        chunks,
        text.len()
    );
    if job["status"] != "succeeded" {
        return Err(job.to_string());
    }
    Ok(())
}
