//! M4 extends the same isolated production-library and mock transport fixtures.
use super::*;
use learnloom_m0::{learning,mastery,backup,store::{Question,Grade,Execution}};
fn grade(score:u8)->Grade{Grade{score,matched:if score==2{vec![0,1]}else if score==1{vec![0]}else{vec![]},missing:if score==0{vec![0,1]}else if score==1{vec![1]}else{vec![]},critical:vec![],noncritical_errors:vec![],explanation:"fixed M4 assessment".into(),standard:"grading_standard_v1".into()}}
fn record(lib:&mut Library,pid:&str,at:i64,score:Option<u8>)->(String,Execution,String){
 let sid=lib.db.session(&format!("history-{at}-{pid}-{}",learnloom_m0::id())).unwrap();
 let e=lib.db.enqueue(&format!("publish-{sid}"),"daily",&sid,"fixture",&Snapshot::default()).unwrap();lib.db.bind_session(&sid,&e).unwrap();lib.db.claim(&e).unwrap();let c=lib.db.call(&e).unwrap();
 let q=Question{point_id:pid.into(),prompt:format!("fixed {pid} {at}"),reference:"fixture".into(),explanation:"fixture teaching relation".into(),required:vec!["a".into(),"b".into()],critical_errors:vec![]};
 let qid=lib.db.publish_questions(&e,&c,&sid,&[q]).unwrap().remove(0);
 let(a,e)=lib.db.submit(&qid,&format!("submit-{qid}"),"fixed answer",&format!("{at:020}"),&Snapshot::default()).unwrap();lib.db.claim(&e).unwrap();let c=lib.db.call(&e).unwrap();
 if let Some(s)=score{assert!(lib.db.grade(&e,&c,&a,&grade(s),"99999999999999999999").unwrap());assert!(!lib.db.grade(&e,&c,&a,&grade(s),"later duplicate").unwrap());}else{lib.db.stop(&e,"failed").unwrap();}
 (a,e,c)
}
fn ids(lib:&Library)->Vec<String>{lib.db.conn.prepare("SELECT id FROM points ORDER BY name,id").unwrap().query_map([],|r|r.get(0)).unwrap().collect::<std::result::Result<_,_>>().unwrap()}
#[test]
fn chronology_late_grade_failure_ties_projection_and_restart(){
 let t=tempfile::tempdir().unwrap();let root=t.path().join("whole library");let mut lib=Library::open(&root,"M4",true).unwrap();fixture::seed(&mut lib).unwrap();let p=ids(&lib).remove(0);
 let(a,e,oldcall)=record(&mut lib,&p,1,None);
 for at in 2..=6{record(&mut lib,&p,at,Some(2));}
 let before=mastery::summary(&lib.db.conn,&p).unwrap();assert_eq!(before.sample_count,5);assert_eq!(before.consecutive_full,5);
 let retry=lib.db.retry(&e,"retry-old",&Snapshot::default()).unwrap();lib.db.claim(&retry).unwrap();let c=lib.db.call(&retry).unwrap();assert!(!lib.db.grade(&e,&oldcall,&a,&grade(0),"late").unwrap());assert!(lib.db.grade(&retry,&c,&a,&grade(0),"late").unwrap());
 let after=mastery::summary(&lib.db.conn,&p).unwrap();assert_eq!(after.recent_score,Some(1.0));assert_eq!(after.consecutive_full,5);assert_eq!(after.effective_count,6);assert_eq!(after.latest_submitted_at.as_deref(),Some("00000000000000000006"));
 record(&mut lib,&p,7,None);assert_eq!(mastery::summary(&lib.db.conn,&p).unwrap(),after);
 // Equal timestamps are ordered by the immutable attempt ID, independently of callback order.
 record(&mut lib,&p,8,Some(0));record(&mut lib,&p,8,Some(2));
 let expected=lib.db.conn.query_row("SELECT a.score FROM attempts a JOIN questions q ON q.id=a.question_id WHERE q.point_id=? AND a.status='graded' ORDER BY a.submitted_at DESC,a.id DESC LIMIT 1",[&p],|r|r.get::<_,u8>(0)).unwrap();
 let final_s=mastery::summary(&lib.db.conn,&p).unwrap();assert_eq!(final_s.consecutive_zero>0,expected==0);
 mastery::rebuild(&mut lib).unwrap();let projected:f64=lib.db.conn.query_row("SELECT recent FROM mastery WHERE point_id=?",[&p],|r|r.get(0)).unwrap();assert_eq!(Some(projected),final_s.recent_score);
 drop(lib);let mut lib=Library::open(&root,"",false).unwrap();lib.recover().unwrap();assert_eq!(mastery::summary(&lib.db.conn,&p).unwrap(),final_s);
}
#[tokio::test]
async fn three_histories_change_real_subsequent_groups_and_pin_policy_inputs(){
 let t=tempfile::tempdir().unwrap();let (url,_,_,task)=mock().await;let app=setup_n(t.path(),&url,10);let root=std::path::PathBuf::from(app.bootstrap().unwrap()["library"]["path"].as_str().unwrap());drop(app);
 let mut evaluations=vec![];let mut lib=Library::open(&root,"",false).unwrap();let ps=ids(&lib);
 for scenario in 0..3{
  if scenario==1{for (i,p) in ps.iter().take(4).enumerate(){for k in 0..2{record(&mut lib,p,100+(i*2+k) as i64,Some(if i<3{0}else{2}));}}}
  if scenario==2{for (i,p) in ps.iter().take(3).enumerate(){for k in 0..5{record(&mut lib,p,200+(i*5+k) as i64,Some(2));}}for(i,p)in ps.iter().skip(4).take(3).enumerate(){for k in 0..2{record(&mut lib,p,300+(i*2+k)as i64,Some(0));}}}
  let day=Fixed(20+scenario);let selected=learning::points(&lib,&day.day().unwrap()).unwrap();assert_eq!(selected.len(),5);assert_eq!(serde_json::to_value(&selected).unwrap(),serde_json::to_value(learning::points(&lib,&day.day().unwrap()).unwrap()).unwrap());
  if scenario==0{assert!(selected.iter().all(|p|p.reason=="unseen"));}else{assert_eq!(selected.iter().filter(|p|p.reason=="weak").count(),3);assert_eq!(selected.iter().filter(|p|p.reason=="unseen").count(),1);assert_eq!(selected.iter().filter(|p|p.reason=="stale").count(),1);assert!(selected.iter().filter(|p|p.reason=="weak").all(|p|p.difficulty=="basic"));}
  drop(lib);let app=AppCore::for_test(t.path()).unwrap();app.seed_test_model(Snapshot{endpoint:url.clone(),..Default::default()},true).unwrap();let s=app.daily_with_clock(&day).unwrap();assert_eq!(wait(&app,&s).await["status"],"succeeded");let actual=app.review_session(s["id"].as_str().unwrap()).unwrap();assert_eq!(actual["actual_count"],5);
  let published=actual["questions"].as_array().unwrap().iter().map(|q|q["point_id"].as_str().unwrap().to_owned()).collect::<Vec<_>>();assert_eq!(published,selected.iter().map(|p|p.point_id.clone()).collect::<Vec<_>>());drop(app);tokio::time::sleep(Duration::from_millis(100)).await;lib=Library::open(&root,"",false).unwrap();
  let snapshots:Vec<Value>={let mut st=lib.db.conn.prepare("SELECT snapshot FROM questions WHERE session_id=? ORDER BY ordinal").unwrap();st.query_map([s["id"].as_str().unwrap()],|r|r.get::<_,String>(0)).unwrap().map(|r|serde_json::from_str(&r.unwrap()).unwrap()).collect()};assert!(snapshots.iter().all(|v|v["policy_input"]["policy_version"]=="policy_v1"));
  evaluations.push(json!({"scenario":(["unseen","three_weak_one_stable","old_weak_now_stable_new_weak"][scenario as usize]),"selected":selected,"published":published,"snapshots":snapshots}));
 }
 assert_ne!(evaluations[1]["published"],evaluations[2]["published"]);
 let out=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../docs/m4/artifacts");std::fs::create_dir_all(&out).unwrap();std::fs::write(out.join("three-histories.json"),serde_json::to_vec_pretty(&json!({"kind":"programmatic_history_and_mock_cloud_production_daily","evaluations":evaluations})).unwrap()).unwrap();task.abort();
}
#[test]
fn online_backup_restores_whole_library_including_wal_learning_and_revisions(){
 let t=tempfile::tempdir().unwrap();let root=t.path().join("live");let mut lib=Library::open(&root,"M4 restore",true).unwrap();fixture::seed(&mut lib).unwrap();let p=ids(&lib).remove(0);record(&mut lib,&p,1,Some(0));let before=mastery::summary(&lib.db.conn,&p).unwrap();
 let dest=t.path().join("frozen snapshot");assert_eq!(backup::create(&lib,&dest).unwrap()["verified"],true);assert!(!dest.join(".learnloom/learnloom.sqlite3-wal").exists());assert!(backup::create(&lib,&root.join("bad")).is_err());assert!(backup::create(&lib,&dest).is_err());
 let restored=t.path().join("restored");backup::restore(&dest,&restored).unwrap();let mut recovered=Library::open(&restored,"",false).unwrap();recovered.recover().unwrap();assert_eq!(mastery::summary(&recovered.db.conn,&p).unwrap(),before);assert_eq!(lib.lists().unwrap(),recovered.lists().unwrap());
 for table in ["sources","revisions","questions","attempts","grading_assessments","messages","answer_drafts"]{let sql=format!("SELECT count(*) FROM {table}");let a:i64=lib.db.conn.query_row(&sql,[],|r|r.get(0)).unwrap();let b:i64=recovered.db.conn.query_row(&sql,[],|r|r.get(0)).unwrap();assert_eq!(a,b,"{table}");}
 std::fs::write(dest.join(".learnloom/learnloom.sqlite3"),b"corrupt").unwrap();assert!(backup::restore(&dest,&t.path().join("bad restore")).is_err());
}
#[test]
fn rubric_rejects_equivalent_items_and_truth_as_critical(){
 use learnloom_m0::explanation;
 let q=Question{point_id:"p".into(),prompt:"计算密度需要什么？".into(),reference:"ρ=m/V".into(),explanation:"先确定质量，再用质量除以体积；交换两者得到不同的比值。".into(),required:vec!["质量除以体积".into(),"ρ=m/V".into()],critical_errors:vec![]};let evidence="密度等于质量除以体积，ρ=m/V。";
 let valid=json!({"valid":true,"reason":"checked","explanation_teaches":true,"explanation_support_quotes":[evidence],"support_quotes":[evidence,evidence],"distinct_items":false,"critical_are_false":true});assert!(explanation::validate_alignment(&valid,&q,evidence).is_err());let mut false_critical=valid;false_critical["distinct_items"]=json!(true);false_critical["critical_are_false"]=json!(false);assert!(explanation::validate_alignment(&false_critical,&q,evidence).is_err());
}
#[test]
fn publication_rejects_copied_answer_and_duplicated_support(){
 use learnloom_m0::explanation;
 let mut q=Question{point_id:"p".into(),prompt:"材料：普通队列遵循先进先出，从队尾入队、从队首出队。请回答队列规则。".into(),reference:"普通队列遵循先进先出，从队尾入队、从队首出队。".into(),explanation:"进入与离开分别使用不同端，先进入的元素先离开。".into(),required:vec!["从队尾入队".into(),"从队首出队".into()],critical_errors:vec![]};assert!(explanation::publication(&q).is_err());q.prompt="普通队列在哪一端入队、在哪一端出队？".into();assert!(explanation::publication(&q).is_ok());let raw=json!({"valid":true,"reason":"checked","explanation_teaches":true,"explanation_support_quotes":[q.reference],"distinct_items":true,"critical_are_false":true,"support_quotes":["从队尾入队","从队尾入队"]});assert!(explanation::validate_alignment(&raw,&q,&q.reference).is_err());let mut good=raw;good["support_quotes"]=json!(["从队尾入队","从队首出队"]);assert!(explanation::validate_alignment(&good,&q,&q.reference).is_ok());
}

#[test]
fn publication_rejects_scoring_fact_with_inserted_filler(){let q=Question{point_id:"p".into(),prompt:"数据库事务把多项更改当作一个整体来处理，这种整体体现在哪里？".into(),reference:"多项更改作为一个整体，回滚撤销未提交更改。".into(),explanation:"要分别检查整体性与提交状态。".into(),required:vec!["多项更改作为一个整体处理".into(),"回滚撤销未提交更改".into()],critical_errors:vec![]};assert!(learnloom_m0::explanation::publication(&q).is_err());}

#[tokio::test]
async fn prior_guard_history_is_rechecked_and_valid_question_remains_reusable(){
 let t=tempfile::tempdir().unwrap();let(url,mode,_,task)=mock().await;let app=setup(t.path(),&url);let s=app.daily_with_clock(&Fixed(1)).unwrap();assert_eq!(wait(&app,&s).await["status"],"succeeded");
 let dbpath=std::path::PathBuf::from(app.bootstrap().unwrap()["library"]["path"].as_str().unwrap()).join(".learnloom/learnloom.sqlite3");let db=rusqlite::Connection::open(dbpath).unwrap();db.execute("UPDATE review_questions SET rubric_details=json_set(rubric_details,'$.publication_guard','guard-v1')",[]).unwrap();drop(db);
 mode.store(5,Ordering::SeqCst);let next=app.daily_with_clock(&Fixed(2)).unwrap();assert_eq!(wait(&app,&next).await["status"],"succeeded");let read=app.review_session(next["id"].as_str().unwrap()).unwrap();assert!(read["questions"][0]["reused_from"].is_string());task.abort();
}

#[test]
fn incomplete_choice_stem_is_never_a_short_answer_question(){let q=Question{point_id:"p".into(),prompt:"关于催化剂的影响，下列说法正确的是（ ）".into(),reference:"催化剂降低活化能，不改变平衡。".into(),explanation:"要分别判断反应速率与平衡。".into(),required:vec!["降低活化能".into(),"不改变平衡".into()],critical_errors:vec![]};assert!(learnloom_m0::explanation::publication(&q).is_err());}
