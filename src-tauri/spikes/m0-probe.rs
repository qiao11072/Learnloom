fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--seed-cloud") {
        let path = std::path::Path::new(&args[2]);
        let mut settings = learnloom_m0::config::Settings::load(path).unwrap();
        let reference = format!("m0-ui-smoke-{}", learnloom_m0::id());
        let key = std::env::var("dsh_key").expect("dsh_key not set");
        learnloom_m0::config::write_credential(&reference, &key).unwrap();
        settings.replace(
            "https://api.deepseek.com",
            "deepseek-flash",
            Some(reference),
        );
        settings.save(path).unwrap();
        println!("Isolated UI cloud config seeded; no key logged");
        return;
    }
    if args.get(1).map(String::as_str) == Some("--cleanup-cloud") {
        let path = std::path::Path::new(&args[2]);
        let settings = learnloom_m0::config::Settings::load(path).unwrap();
        let references: std::collections::BTreeSet<_> = settings
            .versions
            .values()
            .filter_map(|v| v.credential_ref.as_ref())
            .filter(|r| r.starts_with("m0-ui-smoke-"))
            .collect();
        for reference in references {
            let _ = learnloom_m0::config::credential(reference)
                .unwrap()
                .delete_credential();
        }
        learnloom_m0::config::Settings::default()
            .save(path)
            .unwrap();
        println!("UI smoke credential removed; cloud defaults restored");
        return;
    }
    if args.get(1).map(String::as_str) == Some("--credential-write") {
        learnloom_m0::config::write_credential(&args[2], "m0-synthetic-credential").unwrap();
        return;
    }
    if args.get(1).map(String::as_str) == Some("--credential-read-delete") {
        let entry = learnloom_m0::config::credential(&args[2]).unwrap();
        assert_eq!(entry.get_password().unwrap(), "m0-synthetic-credential");
        entry.delete_credential().unwrap();
        println!("credential cross-process roundtrip passed");
        return;
    }
    if args.get(1).map(String::as_str) == Some("--crash-publish") {
        let mut db = learnloom_m0::store::Store::open(std::path::Path::new(&args[2])).unwrap();
        let sid = db
            .save_source(
                "crash source",
                "旧版 催化剂 不改变平衡。新版催化剂降低活化能。",
            )
            .unwrap();
        let e = db
            .enqueue(
                "crash-publish",
                "ingest",
                "page",
                "new",
                &Default::default(),
            )
            .unwrap();
        db.claim(&e).unwrap();
        let call = db.call(&e).unwrap();
        let wiki = learnloom_m0::store::Wiki {
            page_id: "page".into(),
            point_id: "point".into(),
            title: "新版".into(),
            scope: "chemistry".into(),
            body: "新版催化剂降低活化能。".into(),
            evidence: "降低活化能".into(),
        };
        std::env::set_var("LEARNLOOM_M0_CRASH_BEFORE_FTS", "1");
        db.publish_wiki(&e, &call, &sid, &wiki, true).unwrap();
        unreachable!();
    }
    let root = args.get(1).cloned().unwrap_or_else(|| ".m0-data".into());
    match learnloom_m0::storage_probe(std::path::Path::new(&root)) {
        Ok(value) => println!("{}", serde_json::to_string_pretty(&value).unwrap()),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    }
}
