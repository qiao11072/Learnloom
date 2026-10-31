//! Opt-in paid evaluation. Credentials are provided by the invoking process only.
use futures_util::{stream, StreamExt};
use learnloom_m0::{
    config::{credential, write_credential, Snapshot},
    id, model,
    store::Question,
    workflow,
};
use serde_json::{json, Value};
use std::{path::PathBuf, time::Duration};
use tokio_util::sync::CancellationToken;

struct CredentialGuard(String);
impl Drop for CredentialGuard {
    fn drop(&mut self) {
        if let Ok(entry) = credential(&self.0) {
            let _ = entry.delete_credential();
        }
    }
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("M0 real evaluation failed: {error}");
        std::process::exit(1);
    }
}
async fn run() -> Result<(), String> {
    let key = std::env::var("dsh_key").map_err(|_| "CREDENTIAL_UNAVAILABLE: dsh_key")?;
    let reference = format!("m0-real-{}", id());
    write_credential(&reference, &key)?;
    drop(key);
    let _guard = CredentialGuard(reference.clone());
    let snapshot = Snapshot {
        endpoint: "https://api.deepseek.com".into(),
        model: "deepseek-flash".into(),
        credential_ref: Some(reference),
        ..Default::default()
    };
    let artifacts = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../docs/spikes/artifacts");
    let prior = std::fs::read(artifacts.join("real-evaluation.json"))
        .ok()
        .and_then(|v| serde_json::from_slice::<Value>(&v).ok());
    let prior_tokens = prior
        .as_ref()
        .and_then(|v| v["actual_total_tokens"].as_u64())
        .unwrap_or(0);
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../.m0-data/real-{}", id()));
    if std::env::args().any(|a| a == "--knowledge") {
        let result = workflow::knowledge_probe(&root, &snapshot).await?;
        std::fs::write(
            artifacts.join("real-knowledge.json"),
            serde_json::to_vec_pretty(&result).unwrap(),
        )
        .map_err(|e| e.to_string())?;
        println!(
            "real knowledge probe: sources={}, pages={}, points={}, revisions={}",
            result["sources"], result["pages"], result["points"], result["revisions"]
        );
        return Ok(());
    }
    if std::env::args().any(|a| a == "--long") {
        let prior: Value = serde_json::from_slice(
            &std::fs::read(artifacts.join("real-evaluation.json"))
                .map_err(|_| "Run main evaluation first so budget can be checked")?,
        )
        .map_err(|e| e.to_string())?;
        let text = workflow::long_fixture();
        // At most 128 map + 64 reduce requests, each bounded by C=4096.
        if prior["actual_total_tokens"].as_u64().unwrap_or(1_000_000) + 192 * 4096 > 1_000_000 {
            return Err("TOKEN_BUDGET_EXCEEDED".into());
        }
        let result = workflow::long_document(&root, &snapshot, &text).await?;
        std::fs::write(
            artifacts.join("real-long-document.json"),
            serde_json::to_vec_pretty(&result).unwrap(),
        )
        .map_err(|e| e.to_string())?;
        println!(
            "real long document: calls={}, chunks={}, tokens={}",
            result["actual_calls"], result["chunks"], result["total_tokens"]
        );
        return Ok(());
    }
    if prior_tokens + 143 * 4096 > 1_000_000 {
        return Err("TOKEN_BUDGET_EXCEEDED".into());
    }
    if prior.is_some() {
        std::fs::copy(
            artifacts.join("real-evaluation.json"),
            artifacts.join(format!("real-evaluation-history-{}.json", id())),
        )
        .map_err(|e| e.to_string())?;
    }
    let chain = workflow::chain(&root, &snapshot).await?;
    std::fs::write(
        artifacts.join("real-chain.json"),
        serde_json::to_vec_pretty(&chain).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    println!("real chain persisted; model=deepseek-flash; no credentials logged");
    let cases: Vec<Value> = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tests/fixtures/learning-cases.json"
    )))
    .unwrap();
    let mut tasks = vec![];
    for (index, case) in cases.iter().enumerate() {
        let question = Question {
            point_id: format!("p{index}"),
            prompt: case["prompt"].as_str().unwrap().to_string(),
            reference: case["answers"][0].as_str().unwrap().into(),
            explanation: case["evidence"].as_str().unwrap().into(),
            required: serde_json::from_value(case["required"].clone()).unwrap(),
            critical_errors: serde_json::from_value(case["critical_errors"].clone()).unwrap(),
        };
        for variant in 0..3 {
            tasks.push(("question".to_string(),index,variant,0,workflow::QUESTION_PROMPT.to_string(),json!({"point_id":question.point_id,"evidence":case["evidence"],"variant":variant,"language":"zh-CN"}),None));
        }
        for answer in 0..3 {
            for repetition in 0..3 {
                tasks.push((
                    "grade".to_string(),
                    index,
                    answer,
                    repetition,
                    workflow::GRADE_PROMPT.to_string(),
                    workflow::grade_input(
                        &question,
                        case["answers"][answer].as_str().unwrap(),
                        case["evidence"].as_str().unwrap(),
                    ),
                    Some(question.clone()),
                ));
            }
        }
    }
    for index in 0..20 {
        tasks.push(("refusal".to_string(),index,0,0,"Return JSON only {answer_kind:string,answer:string,references:string[]}. Answer using ONLY supplied evidence. If insufficient return answer_kind=insufficient_context, explain lack of evidence, references=[]. Do not use outside knowledge.".into(),json!({"question":format!("{index}：没有给出资料的行星X的质量是多少？"),"evidence":"本知识库只有催化剂降低活化能的资料。"}),None));
    }
    // 143 calls including the chain. Reserve the entire declared context per call.
    // No automatic retries in this evaluator: worst-case declared reservation < 1,000,000.
    let reserved = (tasks.len() + 3) * snapshot.context;
    if reserved > 1_000_000 {
        return Err("TOKEN_BUDGET_EXCEEDED".into());
    }
    let results:Vec<Value>=stream::iter(tasks).map(|(kind,index,variant,repetition,system,input,q)| {
        let snapshot=snapshot.clone();async move {
            let mut request=input.clone();let mut attempts=vec![];let mut final_record=json!(null);let mut total=0u64;
            for correction in 0..2 {
                let reply=model::generate(&snapshot,&system,&request,Duration::from_secs(120),CancellationToken::new()).await;
                let record=match reply {
                    Ok(reply)=>{
                        total+=reply.usage["total_tokens"].as_u64().unwrap_or(0);
                        let raw_output:Value=serde_json::from_str(&reply.content).unwrap();let output=if kind=="grade"{workflow::assessment_grade(&reply.content,q.as_ref().unwrap(),input["answer"].as_str().unwrap()).map(|g|serde_json::to_value(g).unwrap()).unwrap_or(json!(null))}else{raw_output.clone()};
                        let valid=match kind.as_str(){"question"=>serde_json::from_value::<Question>(output.clone()).map(|q|q.validate().is_ok()&&q.point_id==format!("p{index}")).unwrap_or(false),"grade"=>!output.is_null(),_=>output["answer_kind"]=="insufficient_context"&&output["references"]==json!([])};
                        json!({"kind":kind,"case":index,"variant":variant,"repetition":repetition,"expected_score":if kind=="grade"{Some([2,1,0][variant])}else{None},"input":input,"raw_output":raw_output,"output":output,"structurally_valid":valid,"usage":reply.usage,"latency_ms":reply.elapsed_ms,"response_model":reply.model,"config_revision":reply.config_revision})
                    },
                    Err(error)=>json!({"kind":kind,"case":index,"variant":variant,"repetition":repetition,"expected_score":if kind=="grade"{Some([2,1,0][variant])}else{None},"input":input,"error":error,"structurally_valid":false})
                };
                attempts.push(record.clone());final_record=record;
                if final_record["structurally_valid"]==true||correction==1 {break;}
                request["schema_correction"]=json!("Prior output invalid. Use exact requested schema, R*/C* IDs, partition ALL required IDs including incorrect facts as missing. No extra fields. Question requires supplied point_id and 2-4 required entries. Do not change the answer or invent facts.");
            }
            final_record["attempts"]=json!(attempts);final_record["total_tokens"]=json!(total);final_record
        }
    }).buffer_unordered(4).collect().await;
    let grade_rows: Vec<_> = results.iter().filter(|r| r["kind"] == "grade").collect();
    let matched = grade_rows
        .iter()
        .filter(|r| r["structurally_valid"] == true && r["output"]["score"] == r["expected_score"])
        .count();
    let mut matrix = [[0usize; 3]; 3];
    for row in &grade_rows {
        if let (Some(a), Some(b)) = (
            row["expected_score"].as_u64(),
            row["output"]["score"].as_u64(),
        ) {
            if b <= 2 {
                matrix[a as usize][b as usize] += 1;
            }
        }
    }
    let consistent = (0..10)
        .flat_map(|case| (0..3).map(move |variant| (case, variant)))
        .filter(|(case, variant)| {
            let rows: Vec<_> = grade_rows
                .iter()
                .filter(|r| r["case"] == *case && r["variant"] == *variant)
                .collect();
            rows.len() == 3
                && rows.iter().all(|r| {
                    r["structurally_valid"] == true
                        && r["output"]["score"] == rows[0]["output"]["score"]
                })
        })
        .count();
    let chain_usage: u64 = chain["calls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["usage"]["total_tokens"].as_u64().unwrap_or(0))
        .sum();
    let usage: u64 = results
        .iter()
        .map(|r| r["total_tokens"].as_u64().unwrap_or(0))
        .sum::<u64>()
        + chain_usage;
    let report = json!({"mode":"real","provider":"DeepSeek","requested_model":"deepseek-flash","api_model_revision_note":"API listing has no dated 1017 ID; alias version is not independently pinned","context_cap_used":4096,"thinking":"disabled","temperature":0,"prompt_version":"assessment-v6-quoted-evidence-server-score","question_prompt_version":"question-v2","question_prompt_sha256":learnloom_m0::hash(workflow::QUESTION_PROMPT),"budget_authorized_tokens":1000000,"declared_worst_case_reserved_tokens":reserved,"prior_total_tokens":prior_tokens,"run_tokens":usage,"actual_total_tokens":usage+prior_tokens,"request_count":results.len()+3,"grading_standard":"grading_standard_v1","label_status":"agent-authored expected labels; independent human review pending","grade_accuracy":matched as f64/90.0,"grade_matching":matched,"grade_attempts":90,"repeat_consistency":consistent as f64/30.0,"confusion_expected_rows_predicted_columns":matrix,"question_structural_pass":results.iter().filter(|r|r["kind"]=="question"&&r["structurally_valid"]==true).count(),"refusal_pass":results.iter().filter(|r|r["kind"]=="refusal"&&r["structurally_valid"]==true).count(),"results":results});
    std::fs::write(
        artifacts.join("real-evaluation.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    println!(
        "real evaluation: requests={}, tokens={}, grade accuracy={}, repeat consistency={}",
        report["request_count"], usage, report["grade_accuracy"], report["repeat_consistency"]
    );
    Ok(())
}
