//! Opt-in targeted M0 grading after explicit human label confirmation.
use futures_util::{stream, StreamExt};
use learnloom_m0::{
    config::{credential, write_credential, Snapshot},
    hash, id, model,
    store::Question,
    workflow,
};
use serde_json::{json, Value};
use std::{path::PathBuf, time::Duration};
use tokio_util::sync::CancellationToken;
struct CredentialGuard(String);
impl Drop for CredentialGuard {
    fn drop(&mut self) {
        if let Ok(e) = credential(&self.0) {
            let _ = e.delete_credential();
        }
    }
}
fn take_correction(pool: &std::sync::atomic::AtomicUsize) -> bool {
    use std::sync::atomic::Ordering;
    loop {
        let n = pool.load(Ordering::Acquire);
        if n == 0 {
            return false;
        }
        if pool
            .compare_exchange_weak(n, n - 1, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return true;
        }
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn correction_budget_is_shared_and_bounded_under_concurrency() {
        let pool = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(3));
        let handles: Vec<_> = (0..12)
            .map(|_| {
                let p = pool.clone();
                std::thread::spawn(move || super::take_correction(&p))
            })
            .collect();
        assert_eq!(
            handles
                .into_iter()
                .filter_map(|h| h.join().ok())
                .filter(|x| *x)
                .count(),
            3
        );
        assert_eq!(pool.load(std::sync::atomic::Ordering::Acquire), 0);
    }
}
#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("M0 targeted evaluation: {e}");
        std::process::exit(1);
    }
}
async fn run() -> Result<(), String> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let artifacts = root.join("docs/spikes/artifacts");
    let text =
        std::fs::read_to_string(artifacts.join(if std::env::args().any(|a| a == "--fixed") {
            "human-final-acceptance-inputs/fixed-grading-eval-manifest.json"
        } else {
            "human-final-acceptance-inputs/targeted-eval-manifest.json"
        }))
        .map_err(|e| e.to_string())?;
    let manifest: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let journal = std::env::var("M0_REAL_JOURNAL").map_err(|_| "BUDGET_JOURNAL_REQUIRED")?;
    let ledger = std::fs::read_to_string(&journal).map_err(|e| e.to_string())?;
    let mut spent = 0u64;
    let mut pending = std::collections::BTreeMap::new();
    for line in ledger.lines() {
        let v: Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
        match v["kind"].as_str() {
            Some("legacy") => spent += v["tokens"].as_u64().unwrap(),
            Some("start") => {
                pending.insert(
                    v["id"].as_str().unwrap().to_owned(),
                    v["reserved"].as_u64().unwrap(),
                );
            }
            Some("finish") => {
                pending.remove(v["id"].as_str().unwrap());
                spent += v["charged_tokens"].as_u64().unwrap();
            }
            _ => {}
        }
    }
    if !pending.is_empty() {
        return Err("OTHER_EVALUATION_IN_FLIGHT".into());
    }
    if spent + manifest["max_reserved_tokens"].as_u64().unwrap() > 1_000_000 {
        return Err("TOKEN_BUDGET_EXCEEDED".into());
    }
    let key = std::env::var("dsh_key").map_err(|_| "CREDENTIAL_UNAVAILABLE")?;
    let reference = format!("m0-human-{}", id());
    write_credential(&reference, &key)?;
    drop(key);
    let _guard = CredentialGuard(reference.clone());
    let snapshot = Snapshot {
        endpoint: "https://api.deepseek.com".into(),
        model: "deepseek-flash".into(),
        credential_ref: Some(reference),
        ..Default::default()
    };
    let tasks = manifest["tasks"].as_array().unwrap().clone();
    let correction_pool = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(
        manifest["max_corrections_per_batch"]
            .as_u64()
            .unwrap_or(tasks.len() as u64) as usize,
    ));
    let rows:Vec<Value>=stream::iter(tasks).map(|task|{let snapshot=snapshot.clone();let correction_pool=correction_pool.clone();async move{
  let q:Question=serde_json::from_value(task["question"].clone()).unwrap();let answer=task["answer"].as_str().unwrap();let mut input=workflow::grade_input(&q,answer,task["source"].as_str().unwrap());let mut attempts=vec![];let mut grade=None;
  for correction in 0..2{
   let record=match model::generate(&snapshot,workflow::GRADE_PROMPT,&input,Duration::from_secs(120),CancellationToken::new()).await{
    Ok(reply)=>{let parsed=workflow::assessment_grade(&reply.content,&q,answer);let validation=parsed.as_ref().err().cloned();if let Ok(g)=parsed{grade=Some(g);}
     json!({"raw_content":reply.content,"raw_output":serde_json::from_str::<Value>(&reply.content).ok(),"canonical":grade.as_ref(),"validation_error":validation,"usage":reply.usage,"latency_ms":reply.elapsed_ms,"response_model":reply.model,"config_revision":reply.config_revision})},
    Err(e)=>json!({"error":e,"canonical":null})
   };
   attempts.push(record);if grade.is_some(){break;}if correction==0{if !take_correction(&correction_pool){break;}input["correction_error"]=json!("Use exact schema, copy R/C IDs, partition all R IDs, quote actual answer text. No score. Omission is not critical.");}
  }
  json!({"task":task,"structurally_valid":grade.is_some(),"matched_human_score":grade.as_ref().map(|g|Some(g.score as u64)==task["expected"].as_u64()).unwrap_or(false),"canonical":grade,"attempts":attempts})
 }}).buffer_unordered(3).collect().await;
    for (group, filename) in [
        ("supplement", "real-human-quality-supplement.json"),
        ("revisions", "real-human-question-revisions.json"),
        ("fixed", "real-human-fixed-grading-v6.json"),
    ] {
        let selected: Vec<_> = rows
            .iter()
            .filter(|r| r["task"]["group"] == group)
            .collect();
        if selected.is_empty() {
            continue;
        }
        let mut matrix = [[0u32; 3]; 3];
        for row in &selected {
            if let Some(score) = row["canonical"]["score"].as_u64() {
                matrix[row["task"]["expected"].as_u64().unwrap() as usize][score as usize] += 1;
            }
        }
        let ids: std::collections::BTreeSet<_> = selected
            .iter()
            .map(|r| r["task"]["id"].as_str().unwrap())
            .collect();
        let consistent = ids
            .iter()
            .filter(|id| {
                let same: Vec<_> = selected
                    .iter()
                    .filter(|r| r["task"]["id"] == **id)
                    .collect();
                same.iter().all(|r| {
                    r["structurally_valid"] == true
                        && r["canonical"]["score"] == same[0]["canonical"]["score"]
                })
            })
            .count();
        let rpc: usize = selected
            .iter()
            .map(|r| r["attempts"].as_array().unwrap().len())
            .sum();
        let tokens: u64 = selected
            .iter()
            .flat_map(|r| r["attempts"].as_array().unwrap())
            .map(|a| a["usage"]["total_tokens"].as_u64().unwrap_or(0))
            .sum();
        let report = json!({"mode":"real-cloud-targeted","group":group,"provider":"DeepSeek","model_alias":"deepseek-flash","grading_standard":"grading_standard_v1","grade_prompt_version":"assessment-v6","grade_prompt_sha256":hash(workflow::GRADE_PROMPT),"manifest_sha256":hash(&text),"human_labels_confirmed_before_calls":true,"human_feedback":manifest["human_feedback"],"requests":selected.len(),"actual_rpc":rpc,"tokens":tokens,"valid":selected.iter().filter(|r|r["structurally_valid"]==true).count(),"correct":selected.iter().filter(|r|r["matched_human_score"]==true).count(),"confusion_expected_rows_predicted_columns":matrix,"distinct_answers":ids.len(),"consistent_answers":consistent,"separate_report_from_original_90":true,"independent_blind_test":false,"question_v2_generation_run":false,"results":selected});
        let path = artifacts.join(filename);
        if path.exists() {
            std::fs::copy(
                &path,
                artifacts.join(format!("{group}-history-{}.json", id())),
            )
            .map_err(|e| e.to_string())?;
        }
        std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap())
            .map_err(|e| e.to_string())?;
        println!(
            "{group}: correct={}/{}, valid={}, RPC={rpc}, tokens={tokens}",
            report["correct"], report["requests"], report["valid"]
        );
    }
    Ok(())
}
