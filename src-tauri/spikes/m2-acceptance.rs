// Explicit evaluation helper. Paid actions are separate from local search measurement.
mod m2_fixture;
use learnloom_m0::{
    app::AppCore,
    config::Snapshot,
    id,
    knowledge::{Library, PreparedPage, PreparedPoint},
    retrieval, Result,
};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
fn write(path: &Path, v: &Value) -> Result<()> {
    std::fs::write(path, serde_json::to_vec_pretty(v).unwrap()).map_err(|e| e.to_string())
}
#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let action = args.get(1).map(String::as_str).unwrap_or("search");
    let root = PathBuf::from(args.get(2).ok_or("root required")?);
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let kb = root.join("knowledge");
    if action == "search" {
        let mut lib = Library::open(&kb, "M2 检索固定集", true)?;
        let ids = m2_fixture::seed(&mut lib)?;
        let f = m2_fixture::fixture();
        let mut rows = vec![];
        let mut found = 0;
        for q in f["answerable"].as_array().unwrap() {
            let start = Instant::now();
            let r = retrieval::search(&lib, q["query"].as_str().unwrap(), 5)?;
            let hit = r
                .items
                .iter()
                .any(|h| h.page_id == ids[q["page"].as_u64().unwrap() as usize]);
            found += hit as usize;
            rows.push(json!({"query":q,"found":hit,"elapsed_ms":start.elapsed().as_secs_f64()*1000.,"result":r}));
        }
        let raw=(0..90).map(|i|format!("独立资料第{i}段。学习笔记包含用于性能验证的说明文字，不作为固定问题的相关依据。\n")).collect::<String>();
        for batch in 0..10 {
            let sid = lib.save_source(
                &format!("性能资料{batch}"),
                &raw,
                "text",
                &format!("bench{batch}"),
            )?["source_id"]
                .as_str()
                .unwrap()
                .to_owned();
            let e = lib.db.enqueue(
                &format!("bench{batch}"),
                "organize",
                &sid,
                &raw,
                &Snapshot::default(),
            )?;
            lib.db.claim(&e)?;
            let call = lib.db.call(&e)?;
            let mut pages = vec![];
            for i in 0..99 {
                let pid = id();
                lib.db
                    .conn
                    .execute(
                        "INSERT INTO run_page_bases VALUES(?1,?2,NULL)",
                        rusqlite::params![e.run, pid],
                    )
                    .map_err(|e| e.to_string())?;
                pages.push(PreparedPage {
                    id: pid,
                    title: format!("性能资料{batch}-{i}"),
                    scope: "性能".into(),
                    definition: "性能".into(),
                    body: raw.clone(),
                    base: None,
                    points: vec![PreparedPoint {
                        id: id(),
                        name: "性能".into(),
                        definition: "性能".into(),
                        evidence: "学习笔记".into(),
                    }],
                    links: vec![],
                });
            }
            lib.publish(&e, &call, &sid, &pages, false)?;
        }
        let mut timings = vec![];
        for _ in 0..4 {
            for q in f["answerable"].as_array().unwrap() {
                let start = Instant::now();
                retrieval::search(&lib, q["query"].as_str().unwrap(), 5)?;
                timings.push(start.elapsed().as_secs_f64() * 1000.);
            }
        }
        timings.sort_by(f64::total_cmp);
        let pages: i64 = lib
            .db
            .conn
            .query_row("SELECT count(*) FROM pages", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        let bytes:i64=lib.db.conn.query_row("SELECT sum(length(CAST(c.text AS BLOB))) FROM chunks c JOIN pages p ON p.id=c.page_id AND p.current_revision=c.revision_id",[],|r|r.get(0)).map_err(|e|e.to_string())?;
        let report = json!({"type":"local_retrieval","fixed":m2_fixture::fixture()["description"],"recall_at_5":found as f64/30.,"found":found,"queries":30,"rows":rows,"benchmark":{"pages":pages,"body_bytes":bytes,"runs":timings.len(),"p95_ms":timings[(timings.len()*95/100).min(timings.len()-1)],"max_ms":timings.last()}});
        write(&root.join("search-results.json"), &report)?;
        println!("{}", json!({"found":found,"benchmark":report["benchmark"]}));
        return Ok(());
    }
    if action == "quality-seed" {
        let mut lib = Library::open(&kb, "M2 问答固定集", true)?;
        let ids = m2_fixture::seed(&mut lib)?;
        write(&root.join("page-ids.json"), &json!(ids))?;
        return Ok(());
    }
    let profile = root.join("profile");
    let app = AppCore::new(&profile)?;
    if app.bootstrap()?["library"].is_null() {
        let token = app.grant_directory(&kb)?["token"]
            .as_str()
            .unwrap()
            .to_owned();
        app.open_library(&token, "M2", false)?;
    }
    if action == "cloud-seed" {
        let key = std::env::var("dsh_key").map_err(|_| "dsh_key not available")?;
        let rev = app.bootstrap()?["settings"]["config_revision"].clone();
        app.save_model(&json!({"endpoint":"https://api.deepseek.com","model":"deepseek-flash","key":key,"context":4096,"expected_config_revision":rev}))?;
        let rev = app.bootstrap()?["settings"]["config_revision"]
            .as_u64()
            .unwrap();
        let check = app.check_connection(rev).await?;
        write(&root.join("connection.json"), &check)?;
        if check["passed"] != true {
            return Err(format!("{}", check["error"]));
        }
        app.setup_action("complete")?;
        println!("Cloud configured with system credential reference; connection passed.");
        return Ok(());
    }
    if action != "quality" && action != "quality-retest" && action != "quality-final" {
        return Err("unknown action".into());
    }
    let fixture = m2_fixture::fixture();
    let ids: Value = serde_json::from_slice(
        &std::fs::read(root.join("page-ids.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let output = root.join(if action == "quality-final" {
        "quality-final.json"
    } else if action == "quality-retest" {
        "quality-retest.json"
    } else {
        "quality-results.json"
    });
    let mut rows: Vec<Value> = if output.exists() {
        serde_json::from_slice(&std::fs::read(&output).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?
    } else {
        vec![]
    };
    let questions = fixture["answerable"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| {
            (
                x["query"].as_str().unwrap().to_owned(),
                false,
                Some(x["page"].as_u64().unwrap() as usize),
            )
        })
        .chain(
            fixture["unanswerable"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| (x.as_str().unwrap().to_owned(), true, None)),
        )
        .collect::<Vec<_>>();
    for (i, (question, insufficient, expected)) in questions.iter().enumerate() {
        if action == "quality-final" && i < 30 && i != 18 {
            continue;
        }
        if action == "quality-retest" && ![18, 33, 39].contains(&i) {
            continue;
        }
        if rows.iter().any(|r| r["case"] == i) {
            continue;
        }
        let q = app.ask(&json!({"text":question,"request_key":format!("m2-{action}-{i}")}))?;
        let jid = q["job_id"].as_str().unwrap();
        let mut job = app.job(jid)?;
        for _ in 0..660 {
            if !["queued", "running"].contains(&job["status"].as_str().unwrap()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
            job = app.job(jid)?;
        }
        let message = app.message(q["message_id"].as_str().unwrap())?;
        let valid = message.references.iter().all(|r| {
            app.page(&r.page_id, Some(&r.revision_id))
                .ok()
                .is_some_and(|p| p["body"].as_str().unwrap_or("").contains(&r.excerpt))
        });
        let expected_cited = expected.is_none_or(|p| {
            message
                .references
                .iter()
                .any(|r| Some(r.page_id.as_str()) == ids[p].as_str())
        });
        let row = json!({"case":i,"question":question,"expected_insufficient":insufficient,"expected_page":expected.map(|p|ids[p].clone()),"expected_page_cited":expected_cited,"valid_citations":valid,"job":job,"message":message});
        println!(
            "case {i}: {} / {}",
            row["job"]["status"], row["message"]["answer_kind"]
        );
        rows.push(row);
        write(&output, &json!(rows))?;
    }
    Ok(())
}
