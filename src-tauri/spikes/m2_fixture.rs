use learnloom_m0::{
    config::Snapshot,
    id,
    knowledge::{Library, PreparedPage, PreparedPoint},
    Result,
};
use serde_json::Value;
pub fn fixture() -> Value {
    serde_json::from_str(include_str!("../../tests/fixtures/m2-retrieval.json")).unwrap()
}
pub fn seed(lib: &mut Library) -> Result<Vec<String>> {
    let f = fixture();
    let entries = f["pages"].as_array().unwrap();
    let raw = entries
        .iter()
        .map(|p| p["body"].as_str().unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    let sid = lib.save_source("M2 固定检索资料", &raw, "text", "m2-fixture")?["source_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let e = lib.db.enqueue(
        "m2-fixture-publish",
        "organize",
        &sid,
        &raw,
        &Snapshot::default(),
    )?;
    lib.db.claim(&e)?;
    let call = lib.db.call(&e)?;
    let ids = (0..entries.len()).map(|_| id()).collect::<Vec<_>>();
    let mut pages = vec![];
    for (i, p) in entries.iter().enumerate() {
        lib.db
            .conn
            .execute(
                "INSERT INTO run_page_bases VALUES(?1,?2,NULL)",
                rusqlite::params![e.run, ids[i]],
            )
            .map_err(|e| e.to_string())?;
        let body = p["body"].as_str().unwrap();
        let title = p["title"].as_str().unwrap();
        pages.push(PreparedPage {
            id: ids[i].clone(),
            title: title.into(),
            scope: "M2固定资料".into(),
            definition: title.into(),
            body: body.into(),
            base: None,
            points: vec![PreparedPoint {
                id: id(),
                name: title.into(),
                definition: title.into(),
                evidence: body.into(),
            }],
            links: vec![],
        });
    }
    lib.publish(&e, &call, &sid, &pages, false)?;
    Ok(ids)
}
