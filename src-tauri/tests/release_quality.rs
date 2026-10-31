//! Replay actual retained M4 responses without another provider call.
use learnloom_m0::{explanation, store::Question};
use serde_json::Value;

#[test]
#[ignore = "Requires local historical acceptance evidence; excluded from public source"]
fn retained_m4_alignment_results_are_not_semantic_proof() {
    let cases: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/release/artifacts/quality-replay-inputs.json")).expect("local historical evidence is required")).unwrap();
    assert_eq!(cases.len(), 4);
    for case in cases {
        let q: Question = serde_json::from_value(case["question"].clone()).unwrap();
        let evidence = case["evidence"].as_str().unwrap();
        let raw = &case["alignment"];
        assert_eq!(raw["valid"], true); // The model approved even the rejected cases.
        let result = explanation::validate_alignment(raw, &q, evidence);
        match case["point"].as_str().unwrap() {
            "缓存命中" | "密度" => assert!(result.is_ok(), "{result:?}"),
            "欧姆定律" => assert!(result.unwrap_err().starts_with("INVALID_EXPLANATION: evidence quote invalid")),
            "算术平均数" => assert!(result.unwrap_err().starts_with("INVALID_EVIDENCE: rubric support invalid")),
            other => panic!("unexpected point {other}"),
        }
    }
}

#[test]
#[ignore = "Requires local historical acceptance evidence; excluded from public source"]
fn repaired_quotes_show_remaining_semantic_boundary() {
    let cases: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/release/artifacts/quality-replay-inputs.json")).expect("local historical evidence is required")).unwrap();
    let case = cases.iter().find(|c| c["point"] == "欧姆定律").unwrap();
    let q: Question = serde_json::from_value(case["question"].clone()).unwrap();
    let evidence = case["evidence"].as_str().unwrap();
    let mut raw = case["alignment"].clone();
    // Fixing just the first failure must not admit a second fabricated quote.
    raw["explanation_support_quotes"] = serde_json::json!(["电压等于电流乘电阻：U=IR。"]);
    assert!(explanation::validate_alignment(&raw, &q, evidence).unwrap_err().starts_with("INVALID_EVIDENCE"));
    // Deliberately dishonest reviewer with all structurally valid quotes: current
    // validation cannot prove the numerical question/answer is in the evidence.
    // Keep this passing counterexample explicit; do not claim a semantic fix.
    raw["support_quotes"] = serde_json::json!(["电压等于电流乘电阻：U=IR。", "对温度等条件不变的欧姆导体"]);
    assert!(explanation::validate_alignment(&raw, &q, evidence).is_ok());
}
