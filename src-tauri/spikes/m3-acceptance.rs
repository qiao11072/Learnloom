//! Opt-in M3 cloud verification against the isolated M2 desktop knowledge base.
use learnloom_m0::{
    app::AppCore,
    learning::{Clock, Day},
    Result,
};
use serde_json::{json, Value};
use std::{path::PathBuf, time::Duration};
struct Fixed(i64);
impl Clock for Fixed {
    fn day(&self) -> Result<Day> {
        Ok(Day {
            date: format!("2026-11-{:02}", self.0),
            timezone: "Asia/Shanghai (controlled acceptance clock)".into(),
            offset_minutes: 480,
            utc_ms: 1793462400000 + (self.0 - 1) * 86400000,
        })
    }
}
async fn wait(app: &std::sync::Arc<AppCore>, s: &Value) -> Result<Value> {
    for _ in 0..2400 {
        let j = app.job(s["job_id"].as_str().ok_or("missing job")?)?;
        if !["queued", "running"].contains(&j["status"].as_str().unwrap_or("")) {
            return Ok(j);
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Err("TIMEOUT".into())
}
#[tokio::main]
async fn main() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let action = std::env::args().nth(1).unwrap_or("daily".into());
    if std::env::var("M0_REAL_JOURNAL").is_err() {
        return Err("BUDGET_JOURNAL_REQUIRED".into());
    }
    let profile = std::env::var("LEARNLOOM_PROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| root.join(".m3-data/桌面 验收/profile"));
    let app = AppCore::new(&profile)?;
    let out = root.join("docs/m3/artifacts");
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    if action == "regression" || action == "regression-final" {
        regression(&root, &out, &profile, action == "regression-final").await?;
    } else if action == "quality" || action == "quality-final" || action == "quality-verified" {
        for day in 1..=6 {
            let path = out.join(format!("real-{action}-day-{day}.json"));
            if path.exists() {
                continue;
            }
            let s = app.daily_with_clock(&Fixed(day))?;
            let job = wait(&app, &s).await?;
            let s = app.review_session(s["id"].as_str().unwrap())?;
            std::fs::write(path,serde_json::to_vec_pretty(&json!({"kind":"real_cloud_production_review","controlled_day":day,"job":job,"session":s})).unwrap()).map_err(|e|e.to_string())?;
            println!("day {day}: {} / {}", job["status"], s["actual_count"]);
        }
    } else if action == "daily" {
        let s = app.daily()?;
        let job = wait(&app, &s).await?;
        std::fs::write(
            out.join(format!(
                "real-daily-{}.json",
                std::env::var("M3_ARTIFACT_TAG").unwrap_or_else(|_| learnloom_m0::id())
            )),
            serde_json::to_vec_pretty(
                &json!({"job":job,"session":app.review_session(s["id"].as_str().unwrap())?}),
            )
            .unwrap(),
        )
        .map_err(|e| e.to_string())?;
        println!("daily {}", job["status"]);
    } else if action == "answer" {
        let qid = std::env::args().nth(2).ok_or("question id required")?;
        let path = PathBuf::from(std::env::args().nth(3).ok_or("answer file required")?);
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let a = app.submit_review(
            &json!({"question_id":qid,"answer":text,"submission_key":format!("m3-real-{qid}")}),
        )?;
        let job = wait(&app, &a).await?;
        std::fs::write(
            out.join(format!("real-answer-{qid}.json")),
            serde_json::to_vec_pretty(&json!({"attempt":a,"job":job})).unwrap(),
        )
        .map_err(|e| e.to_string())?;
        println!("answer {}", job["status"]);
    } else {
        return Err("unknown action".into());
    }
    Ok(())
}

async fn regression(
    root: &std::path::Path,
    out: &std::path::Path,
    profile: &std::path::Path,
    final_run: bool,
) -> Result<()> {
    use futures_util::{stream, StreamExt};
    use learnloom_m0::{
        app::AppSettings, learning::contextual_grade, model, store::Question, workflow,
    };
    let settings: AppSettings = serde_json::from_slice(
        &std::fs::read(profile.join("settings.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let snapshot = settings.model.current;
    let manifest: Value = serde_json::from_slice(
        &std::fs::read(root.join(
            "docs/spikes/artifacts/human-final-acceptance-inputs/fixed-grading-eval-manifest.json",
        ))
        .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let prefix = if final_run {
        "grading-final"
    } else {
        "grading"
    };
    let tasks = manifest["tasks"].as_array().ok_or("invalid tasks")?.clone();
    let rows=stream::iter(tasks.into_iter().enumerate().map(|(i,t)|{
  let snapshot=snapshot.clone();let path=out.join(format!("{prefix}-{i:03}.json"));async move{
   if path.exists(){return serde_json::from_slice::<Value>(&std::fs::read(path).map_err(|e|e.to_string())?).map_err(|e|e.to_string())}
   let q:Question=serde_json::from_value(t["question"].clone()).map_err(|e|e.to_string())?;
   let answer=t["answer"].as_str().unwrap();let mut input=workflow::grade_input(&q,answer,t["source"].as_str().unwrap());
   input["language"]=json!(snapshot.language);input["quote_context_rule"]=json!("For EVERY matched/critical hit copy the ENTIRE answer verbatim as quote, including all sentences and punctuation. Identical full-answer quotes are allowed. If genuinely ambiguous return {ungradable:true}; never guess zero.");
   let mut trace=vec![];let mut grade=None;let mut error=String::new();
   for n in 0..2 {match model::generate(&snapshot,workflow::GRADE_PROMPT,&input,Duration::from_secs(120),tokio_util::sync::CancellationToken::new()).await{
    Ok(reply)=>{let raw:Value=serde_json::from_str(&reply.content).map_err(|e|e.to_string())?;trace.push(json!({"raw":raw,"usage":reply.usage}));match contextual_grade(&raw,&q,answer){Ok(g)=>{grade=Some(g);break},Err(e)=>{error=e;input["correction_error"]=json!(error);}}},
    Err(e)=>{error=e;if n==1||!error.starts_with("INVALID_OUTPUT"){break}}
   }}
   let row=json!({"case":t,"grade":grade,"error":if grade.is_some(){""}else{&error},"trace":trace});std::fs::write(path,serde_json::to_vec_pretty(&row).unwrap()).map_err(|e|e.to_string())?;Ok(row)
  }
 })).buffer_unordered(4).collect::<Vec<Result<Value>>>().await;
    let rows = rows.into_iter().collect::<Result<Vec<_>>>()?;
    let valid = rows
        .iter()
        .filter(|r| r["grade"]["score"].is_number())
        .count();
    let matched = rows
        .iter()
        .filter(|r| r["grade"]["score"] == r["case"]["expected"])
        .count();
    let report = json!({"kind":"real_cloud_confirmed_label_regression","cases":rows.len(),"valid":valid,"matched":matched,"rows":rows});
    std::fs::write(
        out.join(format!("{prefix}-summary.json")),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    println!("grading: {matched}/{} matched; {valid} valid", rows.len());
    Ok(())
}
