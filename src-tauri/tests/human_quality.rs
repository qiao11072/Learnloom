//! M0 evidence replay and synthetic boundary audit. Never contacts a model.
use learnloom_m0::{hash, store::Question, workflow::assessment_grade};
use serde_json::{json, Value};
use std::path::PathBuf;
fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}
fn indexed(v: &Value, prefix: &str) -> Vec<String> {
    let mut items: Vec<_> = v
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| {
            (
                k.strip_prefix(prefix).unwrap().parse::<usize>().unwrap(),
                v.as_str().unwrap().to_owned(),
            )
        })
        .collect();
    items.sort_by_key(|x| x.0);
    for (i, x) in items.iter().enumerate() {
        assert_eq!(i, x.0);
    }
    items.into_iter().map(|x| x.1).collect()
}
#[test]
#[ignore = "Requires local historical acceptance evidence; excluded from public source"]
fn replay_saved_real_outputs_unchanged() {
    let path = root().join("docs/spikes/artifacts/real-closeout.json");
    let original = std::fs::read_to_string(&path).unwrap();
    let report: Value = serde_json::from_str(&original).unwrap();
    let mut replayed = 0;
    for (i, row) in report["results"].as_array().unwrap().iter().enumerate() {
        if !row["kind"].as_str().unwrap().starts_with("grade") {
            continue;
        }
        let input = &row["input"];
        let q = Question {
            point_id: "replay".into(),
            prompt: input["prompt"].as_str().unwrap().into(),
            reference: input["reference"].as_str().unwrap().into(),
            explanation: input["evidence"].as_str().unwrap().into(),
            required: indexed(&input["required"], "R"),
            critical_errors: indexed(&input["critical_errors"], "C"),
        };
        let grade = assessment_grade(
            &row["raw_output"].to_string(),
            &q,
            input["answer"].as_str().unwrap(),
        )
        .unwrap_or_else(|e| panic!("results[{i}]: {e}"));
        assert_eq!(
            serde_json::to_value(grade).unwrap(),
            row["output"],
            "results[{i}]"
        );
        replayed += 1;
    }
    assert_eq!(replayed, 98);
    let evidence = json!({"mode":"offline-replay-of-saved-real-output","replayed":replayed,"same_canonical_results":replayed,"source_sha256":hash(&original),"new_model_calls":0,"human_quality_decision":"待确认","note":"Proves reproducibility of extraction-to-score mapping, not semantic correctness of extraction."});
    std::fs::write(
        root().join("docs/spikes/artifacts/human-review-rust-replay.json"),
        serde_json::to_vec_pretty(&evidence).unwrap(),
    )
    .unwrap();
}
#[test]
fn supplement_contract_boundaries_and_semantic_limits() {
    let suite: Value = serde_json::from_str(
        &std::fs::read_to_string(root().join("tests/fixtures/human-quality-supplement.json"))
            .unwrap(),
    )
    .unwrap();
    let q: Question = serde_json::from_value(suite["question"].clone()).unwrap();
    let mut results = vec![];
    let assessment = |matched: Value, missing: Value, critical: Value| json!({"matched":matched,"missing":missing,"critical":critical,"noncritical_errors":[],"explanation":"Synthetic extraction supplied by test, not a model judgement."});
    for row in suite["cases"].as_array().unwrap() {
        let id = row["id"].as_str().unwrap();
        let a = match id {
            "H01" | "H08" => assessment(
                json!([{"id":"R0","quote":"降低活化能"},{"id":"R1","quote":"不改变平衡位置"}]),
                json!([]),
                json!([]),
            ),
            "H02" => assessment(
                json!([{"id":"R1","quote":"不改变平衡位置"}]),
                json!(["R0"]),
                json!([{"id":"C2","quote":"不会降低活化能"}]),
            ),
            "H03" => assessment(
                json!([{"id":"R1","quote":"不改变平衡位置"}]),
                json!(["R0"]),
                json!([{"id":"C0","quote":"使平衡向产物移动"}]),
            ),
            "H04" => assessment(
                json!([{"id":"R0","quote":"降低活化能"}]),
                json!(["R1"]),
                json!([]),
            ),
            _ => assessment(json!([]), json!(["R0", "R1"]), json!([])),
        };
        let score = assessment_grade(&a.to_string(), &q, row["answer"].as_str().unwrap())
            .unwrap()
            .score;
        assert_eq!(Some(score as u64), row["proposed_score"].as_u64());
        results.push(json!({"id":id,"mode":"synthetic-correct-extraction-to-Rust","computed_score":score,"label_status":"待确认","real_model_run":false}));
    }
    let fabricated = assessment(
        json!([{"id":"R0","quote":"降低活化能"}]),
        json!(["R1"]),
        json!([]),
    );
    let rejected = assessment_grade(&fabricated.to_string(), &q, "我不知道。").is_err();
    assert!(rejected);
    // The following deliberately wrong extractions expose the semantic boundary.
    // A passing test here documents the limitation; it does NOT assert a quality pass.
    let both = assessment(
        json!([{"id":"R0","quote":"降低活化能"},{"id":"R1","quote":"不改变平衡位置"}]),
        json!([]),
        json!([]),
    );
    let negated = suite["cases"][1]["answer"].as_str().unwrap();
    let denial_score = assessment_grade(&both.to_string(), &q, negated)
        .unwrap()
        .score;
    assert_eq!(denial_score, 2);
    let irrelevant = suite["cases"][5]["answer"].as_str().unwrap();
    let irrelevant_score = assessment_grade(&fabricated.to_string(), &q, irrelevant)
        .unwrap()
        .score;
    assert_eq!(irrelevant_score, 1);
    let lost_critical = assessment(
        json!([{"id":"R1","quote":"不改变平衡位置"}]),
        json!(["R0"]),
        json!([]),
    );
    let contradiction_score = assessment_grade(
        &lost_critical.to_string(),
        &q,
        suite["cases"][2]["answer"].as_str().unwrap(),
    )
    .unwrap()
    .score;
    assert_eq!(contradiction_score, 1);
    let evidence = json!({"mode":"synthetic-contract-and-known-limit-audit","new_model_calls":0,"cases":results,"quote_only_in_source_rejected":rejected,"known_semantic_limits":[{"id":"H02","bad_extraction_score":denial_score,"proposed_semantic_score":0,"reason":"Quote is a substring of a negated claim; parser cannot establish affirmation."},{"id":"H06","bad_extraction_score":irrelevant_score,"proposed_semantic_score":0,"reason":"True substring in irrelevant metalinguistic context is not fact support."},{"id":"H03","bad_extraction_score":contradiction_score,"proposed_semantic_score":0,"reason":"Missing critical detection cannot be reconstructed from IDs alone."}],"real_model_quality":"未验证","human_labels":"待确认"});
    std::fs::write(
        root().join("docs/spikes/artifacts/human-review-contract-audit.json"),
        serde_json::to_vec_pretty(&evidence).unwrap(),
    )
    .unwrap();
}
#[test]
#[ignore = "Requires local historical acceptance evidence; excluded from public source"]
fn saved_scores_match_only_explicitly_confirmed_human_labels() {
    let materials: Value = serde_json::from_str(
        &std::fs::read_to_string(root().join("docs/spikes/artifacts/human-review-materials.json"))
            .unwrap(),
    )
    .unwrap();
    let report: Value = serde_json::from_str(
        &std::fs::read_to_string(root().join("docs/spikes/artifacts/real-closeout.json")).unwrap(),
    )
    .unwrap();
    let labels = materials["confirmed_labels"].as_array().unwrap();
    assert!(!labels.is_empty());
    let mut checked = 0;
    for label in labels {
        let id = label["id"].as_str().unwrap();
        let mut parts = id.strip_prefix('A').unwrap().split('-');
        let case = parts.next().unwrap().parse::<usize>().unwrap() - 1;
        let variant = parts.next().unwrap().parse::<usize>().unwrap() - 1;
        let rows: Vec<_> = report["results"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["kind"] == "grade" && r["case"] == case && r["variant"] == variant)
            .collect();
        assert_eq!(rows.len(), 3);
        for row in rows {
            assert_eq!(row["output"]["score"], label["score"], "{id}");
            checked += 1;
        }
    }
    let evidence = json!({"mode":"saved-real-score-vs-explicit-user-label","confirmed_answer_labels":labels.len(),"saved_scores_checked":checked,"matching":checked,"new_model_calls":0,"full_30_answer_acceptance":labels.len()==30,"note":"Numeric labels only; extraction/citation/explanation review is separate."});
    std::fs::write(
        root().join("docs/spikes/artifacts/human-confirmed-label-replay.json"),
        serde_json::to_vec_pretty(&evidence).unwrap(),
    )
    .unwrap();
}
#[test]
fn revised_question_candidates_and_equivalent_formula_contracts() {
    let suite: Value = serde_json::from_str(
        &std::fs::read_to_string(root().join("tests/fixtures/human-question-revisions.json"))
            .unwrap(),
    )
    .unwrap();
    let mut count = 0;
    for revision in suite["revisions"].as_array().unwrap() {
        let q: Question = serde_json::from_value(revision["question"].clone()).unwrap();
        q.validate().unwrap();
        for row in revision["proposed_answer_cases"].as_array().unwrap() {
            let extraction = json!({"matched":row["matched"],"missing":row["missing"],"critical":row["critical"],"noncritical_errors":[],"explanation":"Synthetic extraction for a manually revised candidate; no model call."});
            let g = assessment_grade(&extraction.to_string(), &q, row["answer"].as_str().unwrap())
                .unwrap();
            assert_eq!(Some(g.score as u64), row["proposed_score"].as_u64());
            count += 1;
        }
    }
    assert_eq!(count, 13);
    std::fs::write(root().join("docs/spikes/artifacts/human-question-revision-contracts.json"),serde_json::to_vec_pretty(&json!({"mode":"synthetic-contract-only","revised_candidates":4,"mapping_cases":count,"new_model_calls":0,"real_generation_and_grading":"未运行","human_final_question_review":"待确认"})).unwrap()).unwrap();
}
