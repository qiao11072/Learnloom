//! Opt-in explanation-only repair of the exact 30 reviewed candidates.
use futures_util::{stream, StreamExt};
use learnloom_m0::{app::AppSettings, explanation, model, store::Question, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{path::PathBuf, time::Duration};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    explanation: String,
}
#[tokio::main]
async fn main() -> Result<()> {
    if std::env::var("M0_REAL_JOURNAL").is_err() {
        return Err("BUDGET_JOURNAL_REQUIRED".into());
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let profile = std::env::var("LEARNLOOM_PROFILE")
        .map(PathBuf::from)
        .map_err(|_| "ISOLATED_PROFILE_REQUIRED")?;
    let config: AppSettings = serde_json::from_slice(
        &std::fs::read(profile.join("settings.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let manifest: Value = serde_json::from_slice(
        &std::fs::read(root.join("docs/m3/artifacts/explanation-revision-inputs.json"))
            .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let verify_edited = std::env::args().any(|a| a == "--verify-edited");
    let edited: Option<Value> = if verify_edited {
        Some(
            serde_json::from_slice(
                &std::fs::read(root.join("docs/m3/artifacts/explanation-edited.json"))
                    .map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?,
        )
    } else {
        None
    };
    let out = root.join(if verify_edited {
        "docs/m3/artifacts/explanations-final"
    } else {
        "docs/m3/artifacts/explanations-v6"
    });
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let retry = std::env::args().any(|a| a == "--retry-failed");
    let rows=stream::iter(manifest["cases"].as_array().ok_or("INVALID_CASES")?.clone().into_iter().map(|case|{
  let snapshot=config.model.current.clone();let out=out.clone();let edit=edited.as_ref().and_then(|v|v.as_array()).and_then(|items|items.iter().find(|r|r["id"]==case["id"])).and_then(|r|r["explanation"].as_str()).map(str::to_owned);async move{
   let id=case["id"].as_str().unwrap();let path=out.join(format!("{id}.json"));
   if path.exists(){let existing:Value=serde_json::from_slice(&std::fs::read(&path).map_err(|e|e.to_string())?).map_err(|e|e.to_string())?;if existing["status"]=="validated"||!retry{return Ok(existing)}
    let archive=out.join(format!("{id}-failed-{}.json",learnloom_m0::id()));std::fs::copy(&path,archive).map_err(|e|e.to_string())?;
   }
   let original:Question=serde_json::from_value(case["question"].clone()).map_err(|e|e.to_string())?;let mut revised=original.clone();
   let evidence=case["point"]["evidence"].as_str().unwrap();
   let mut input=json!({"question":original,"evidence":evidence,"language":snapshot.language});
   let mut trace=vec![];let mut status="failed";let mut error=String::new();
   // Bounded artifact revision: at most two rewrite RPCs and two independent checks.
   for _ in 0..if edit.is_some(){1}else{2}{
    if let Some(text)=&edit{revised.explanation=text.clone();trace.push(json!({"stage":"assistant_edit","explanation":text}));}else{
    match model::generate(&snapshot,explanation::REWRITE_PROMPT,&input,Duration::from_secs(120),tokio_util::sync::CancellationToken::new()).await{
     Ok(reply)=>{trace.push(json!({"stage":"rewrite","raw":reply.content,"usage":reply.usage}));
      match serde_json::from_str::<Reply>(&reply.content){Ok(r)=>revised.explanation=r.explanation,Err(e)=>{error=e.to_string();input["correction_error"]=json!(error);continue}}
     },Err(e)=>{error=e;input["correction_error"]=json!(error);continue}
    }
    }
    if let Err(e)=explanation::distinct(&revised){error=e;input["correction_error"]=json!(error);continue}
    let check_input=json!({"prompt":revised.prompt,"reference":revised.reference,"explanation":revised.explanation,"evidence":evidence,"quote_source_rule":"Every quote MUST be copied verbatim from the evidence field only, NEVER from the explanation. Valid deductions still need source quotes, not deduction text."});
    match model::generate(&snapshot,explanation::CHECK_PROMPT,&check_input,Duration::from_secs(120),tokio_util::sync::CancellationToken::new()).await{
     Ok(reply)=>{let raw:Value=serde_json::from_str(&reply.content).map_err(|e|e.to_string())?;trace.push(json!({"stage":"independent_check","raw":raw,"usage":reply.usage}));
      match explanation::validate_review(&raw,&revised,evidence){Ok(())=>{status="validated";error.clear();break},Err(e)=>{error=e;input["correction_error"]=json!({"error":error,"review":raw});}}
     },Err(e)=>{error=e;input["correction_error"]=json!(error);}
    }
   }
   let row=json!({"id":id,"original_call_id":case["original_call_id"],"status":status,"error":error,"original":original,"revised":revised,"point":case["point"],"trace":trace,"human_decision":null});
   std::fs::write(path,serde_json::to_vec_pretty(&row).unwrap()).map_err(|e|e.to_string())?;println!("{id}: {status}");Ok(row)
  }
 })).buffer_unordered(3).collect::<Vec<Result<Value>>>().await.into_iter().collect::<Result<Vec<_>>>()?;
    let valid = rows.iter().filter(|r| r["status"] == "validated").count();
    let summary = json!({"cases":rows.len(),"validated":valid,"failed":rows.len()-valid,"kind":if verify_edited{"assistant_edits_with_real_cloud_checks"}else{"real_cloud_explanation_only_revision"},"generator":"question-v6","original_fields_fixed":true,"human_approval":false,"rows":rows});
    std::fs::write(
        out.join("summary.json"),
        serde_json::to_vec_pretty(&summary).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    println!("{valid}/{} explanation revisions validated", rows.len());
    Ok(())
}
