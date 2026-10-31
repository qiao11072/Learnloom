use learnloom_m0::{
    config::Snapshot,
    id,
    store::{Store, Wiki},
};
use serde_json::{json, Value};
use std::{path::PathBuf, time::Instant};
fn main() {
    run().unwrap();
}
fn run() -> Result<(), String> {
    let root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("../.m0-data/search-{}", id()));
    let mut db = Store::open(&root)?;
    let cases: Vec<Value> = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tests/fixtures/learning-cases.json"
    )))
    .unwrap();
    let queries = [
        vec!["催化剂", "活化能", "让平衡更快到达"],
        vec!["数据库事务", "回滚", "一组更改撤销"],
        vec!["二分查找", "有序", "每轮排除一半"],
        vec!["欧姆定律", "电压", "电压电流电阻关系"],
        vec!["算术平均数", "总和", "把所有数求和再除以个数"],
        vec!["栈", "栈顶", "后进先出的结构"],
        vec!["队列", "队首", "先进先出的结构"],
        vec!["密度", "质量", "质量体积如何计算"],
        vec!["缓存命中", "缓存", "直接读取而无需源存储"],
        vec!["版本快照", "快照", "原内容更新后仍固定不变"],
    ];
    let start = Instant::now();
    let mut bytes = 0;
    for index in 0..1000 {
        let (title, body) = if index < cases.len() {
            (
                cases[index]["name"].as_str().unwrap().to_string(),
                cases[index]["evidence"].as_str().unwrap().to_string()
                    + &" Background content for corpus size. ".repeat(285),
            )
        } else {
            (
                format!("Filler {index}"),
                format!("Unrelated filler document {index}. ")
                    + &" Neutral background data, no learning concept. ".repeat(225),
            )
        };
        bytes += body.len();
        let sid = db.save_source(&title, &body)?;
        let page = format!("page-{index}");
        let e = db.enqueue(
            &format!("bench-{index}"),
            "ingest",
            &page,
            &body,
            &Snapshot::default(),
        )?;
        db.claim(&e)?;
        let c = db.call(&e)?;
        db.publish_wiki(
            &e,
            &c,
            &sid,
            &Wiki {
                page_id: page,
                point_id: format!("point-{index}"),
                title,
                scope: "benchmark".into(),
                evidence: body[..body.char_indices().nth(8).unwrap().0].into(),
                body,
            },
            false,
        )?;
    }
    let mut results = vec![];
    let mut timings = vec![];
    for (index, qs) in queries.iter().enumerate() {
        for query in qs {
            let time = Instant::now();
            let found = db.search(query)?;
            timings.push(time.elapsed().as_secs_f64() * 1000.0);
            results.push(json!({"query":query,"expected":format!("page-{index}"),"found":found,"hit":found.contains(&format!("page-{index}"))}));
        }
    }
    timings.sort_by(f64::total_cmp);
    let hits = results.iter().filter(|r| r["hit"] == true).count();
    let report = json!({"pages":1000,"text_bytes":bytes,"build_ms":start.elapsed().as_millis(),"queries":30,"hits":hits,"recall_at_5":hits as f64/30.0,"p95_ms":timings[28],"results":results,"scope":"fixed lexical/paraphrase set; not general semantic search proof"});
    let filename = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "search-benchmark.json".into());
    std::fs::write(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../docs/spikes/artifacts")
            .join(filename),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    println!(
        "pages=1000 bytes={bytes} recall@5={} p95={}ms",
        report["recall_at_5"], report["p95_ms"]
    );
    Ok(())
}
