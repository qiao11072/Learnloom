use learnloom_m0::{config::validate_cloud, store::Question, workflow::assessment_grade};
use serde_json::json;
fn q() -> Question {
    Question {
        point_id: "p".into(),
        prompt: "催化剂如何影响活化能和平衡？".into(),
        reference: "降低活化能，不改平衡".into(),
        explanation: "资料".into(),
        required: vec!["降低活化能".into(), "不改平衡".into()],
        critical_errors: vec!["平衡向产物移动".into()],
    }
}
#[test]
fn cloud_settings_reject_local_and_credentials_in_url() {
    for url in [
        "http://localhost:11434/v1",
        "https://localhost/v1",
        "https://127.0.0.1",
        "https://192.168.1.2",
        "https://[::1]",
        "https://[::ffff:127.0.0.1]",
        "https://model.local",
        "https://user:key@api.example.com",
        "https://api.example.com?key=x",
    ] {
        assert!(validate_cloud(url, "m").is_err(), "{url}");
    }
    assert_eq!(
        validate_cloud("https://api.example.com/v1/", "m").unwrap(),
        "https://api.example.com/v1"
    );
    assert!(validate_cloud("https://api.deepseek.com", "").is_err());
}
#[test]
fn assessment_partial_omission_and_critical_quote() {
    let partial = json!({"matched":[{"id":"R0","quote":"降低活化能"}],"missing":["R1"],"critical":[],"noncritical_errors":[],"explanation":"漏答平衡不是错误断言"});
    assert_eq!(
        assessment_grade(&partial.to_string(), &q(), "降低活化能。")
            .unwrap()
            .score,
        1
    );
    let mut wrong_namespace = partial.clone();
    wrong_namespace["matched"] = json!([{"id":"C0","quote":"降低活化能"}]);
    assert!(assessment_grade(&wrong_namespace.to_string(), &q(), "降低活化能。").is_err());
    let mut invented = partial.clone();
    invented["critical"] = json!([{"id":"C0","quote":"平衡向产物移动"}]);
    assert!(assessment_grade(&invented.to_string(), &q(), "降低活化能。").is_err());
    assert_eq!(
        assessment_grade(&invented.to_string(), &q(), "降低活化能，平衡向产物移动。")
            .unwrap()
            .score,
        0
    );
    let mut invalid = partial.clone();
    invalid["missing"] = json!([]);
    assert!(assessment_grade(&invalid.to_string(), &q(), "降低活化能。").is_err());
    invalid = partial;
    invalid["score"] = json!(0);
    assert!(assessment_grade(&invalid.to_string(), &q(), "降低活化能。").is_err());
}

#[test]
fn active_prompts_fit_conservative_4k_budget() {
    let b = learnloom_m0::budget::Budget::new(4096).unwrap();
    for p in [
        learnloom_m0::workflow::WIKI_PROMPT,
        learnloom_m0::workflow::QUESTION_PROMPT,
        learnloom_m0::workflow::GRADE_PROMPT,
    ] {
        b.check(p, "{}", "", "").unwrap();
    }
}
