//! mastery_v1: authoritative graded history ordered by submission, never completion.
use crate::{app::AppCore,hash,knowledge::{now,Library},Result};
use rusqlite::{params,Connection,Transaction};
use serde::{Serialize,Deserialize};
use serde_json::{json,Value};
fn err(e:impl std::fmt::Display)->String{e.to_string()}
#[derive(Clone,Debug,Default,Serialize,Deserialize,PartialEq)]
pub struct Summary{
 pub effective_count:usize,pub sample_count:usize,pub recent_score:Option<f64>,pub weak:Option<bool>,pub consecutive_zero:usize,pub consecutive_full:usize,pub latest_submitted_at:Option<String>,pub input_watermark:String,
}
pub fn summary(db:&Connection,pid:&str)->Result<Summary>{
 let mut st=db.prepare("SELECT a.id,a.score,a.submitted_at FROM attempts a JOIN questions q ON q.id=a.question_id WHERE q.point_id=? AND a.status='graded' ORDER BY a.submitted_at DESC,a.id DESC").map_err(err)?;
 let rows=st.query_map([pid],|r|Ok((r.get::<_,String>(0)?,r.get::<_,u8>(1)?,r.get::<_,String>(2)?))).map_err(err)?.collect::<std::result::Result<Vec<_>,_>>().map_err(err)?;
 let n=rows.len().min(5);let recent=if n==0{None}else{Some(rows.iter().take(n).map(|r|r.1 as f64/2.0).sum::<f64>()/n as f64)};
 Ok(Summary{effective_count:rows.len(),sample_count:n,recent_score:recent,weak:recent.map(|r|r<0.6),consecutive_zero:rows.iter().take_while(|r|r.1==0).count(),consecutive_full:rows.iter().take_while(|r|r.1==2).count(),latest_submitted_at:rows.first().map(|r|r.2.clone()),input_watermark:hash(&rows.iter().map(|r|format!("{}:{}:{}",r.0,r.1,r.2)).collect::<Vec<_>>().join("|"))})
}
pub fn project(tx:&Transaction<'_>,pid:&str)->Result<Summary>{
 let s=summary(tx,pid)?;
 let meta:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='mastery_projection')",[],|r|r.get(0)).map_err(err)?;
 if let Some(recent)=s.recent_score{
  tx.execute("INSERT INTO mastery VALUES(?,?,?,?) ON CONFLICT(point_id) DO UPDATE SET count=excluded.count,recent=excluded.recent,weak=excluded.weak",params![pid,s.sample_count,recent,s.weak]).map_err(err)?;
  if meta{tx.execute("INSERT INTO mastery_projection VALUES(?,'mastery_v1',?,?) ON CONFLICT(point_id) DO UPDATE SET algorithm_version=excluded.algorithm_version,input_watermark=excluded.input_watermark,projection_updated_at=excluded.projection_updated_at",params![pid,s.input_watermark,now()]).map_err(err)?;}
 }else{tx.execute("DELETE FROM mastery WHERE point_id=?",[pid]).map_err(err)?;if meta{tx.execute("DELETE FROM mastery_projection WHERE point_id=?",[pid]).map_err(err)?;}}
 Ok(s)
}
pub fn rebuild(lib:&mut Library)->Result<Value>{rebuild_db(&mut lib.db.conn)}
pub fn rebuild_db(db:&mut Connection)->Result<Value>{
 let tx=db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(err)?;
 let ids={let mut s=tx.prepare("SELECT id FROM points ORDER BY id").map_err(err)?;let rows=s.query_map([],|r|r.get::<_,String>(0)).map_err(err)?.collect::<std::result::Result<Vec<_>,_>>().map_err(err)?;rows};
 for pid in &ids{project(&tx,pid)?;}
 tx.commit().map_err(err)?;Ok(json!({"algorithm_version":"mastery_v1","points":ids.len(),"rebuilt":true}))
}
pub fn list(lib:&Library,filter:&str)->Result<Value>{
 if !["all","weak","unseen","evaluated"].contains(&filter){return Err("INVALID_FILTER".into())}
 let tx=lib.db.conn.unchecked_transaction().map_err(err)?;
 let ids={let mut st=tx.prepare("SELECT p.id,p.name,p.page_id,p.revision_id,w.current_revision,w.title,w.index_state FROM points p JOIN pages w ON w.id=p.page_id ORDER BY p.name,p.id").map_err(err)?;let rows=st.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?,r.get::<_,String>(6)?))).map_err(err)?.collect::<std::result::Result<Vec<_>,_>>().map_err(err)?;rows};
 let mut items=vec![];
 for (pid,name,page,rev,current,title,state) in ids{let s=summary(&tx,&pid)?;let status=if s.weak==Some(true){"weak"}else if s.recent_score.is_none(){"unseen"}else{"evaluated"};if filter!="all"&&filter!=status&&!(filter=="evaluated"&&s.recent_score.is_some()){continue}items.push(json!({"point_id":pid,"name":name,"page_id":page,"revision_id":rev,"current_revision":current,"page_title":title,"index_state":state,"status":status,"performance":s,"few_samples":s.sample_count>0&&s.sample_count<3}));}
 tx.commit().map_err(err)?;Ok(json!({"algorithm_version":"mastery_v1","items":items,"meaning":"最近最多五次有效正式作答得分/2的平均值，不是精确掌握概率；失败不计入。"}))
}
pub fn history(lib:&Library,pid:&str)->Result<Value>{
 let mut st=lib.db.conn.prepare("SELECT a.id,a.answer,a.submitted_at,a.graded_at,a.status,a.score,a.feedback,q.id,q.prompt,q.reference,q.explanation,q.snapshot,q.session_id,s.date,COALESCE(j.error,'') FROM attempts a JOIN questions q ON q.id=a.question_id JOIN sessions s ON s.id=q.session_id LEFT JOIN job_meta j ON j.job_id=a.job_id WHERE q.point_id=? ORDER BY a.submitted_at DESC,a.id DESC").map_err(err)?;
 let rows=st.query_map([pid],|r|Ok(json!({"attempt_id":r.get::<_,String>(0)?,"answer":r.get::<_,String>(1)?,"submitted_at":r.get::<_,String>(2)?,"graded_at":r.get::<_,Option<String>>(3)?,"status":r.get::<_,String>(4)?,"score":r.get::<_,Option<u8>>(5)?,"feedback":r.get::<_,Option<String>>(6)?.and_then(|s|serde_json::from_str::<Value>(&s).ok()),"question_id":r.get::<_,String>(7)?,"prompt":r.get::<_,String>(8)?,"reference":r.get::<_,String>(9)?,"explanation":r.get::<_,String>(10)?,"context":serde_json::from_str::<Value>(&r.get::<_,String>(11)?).unwrap_or(Value::Null),"session_id":r.get::<_,String>(12)?,"date":r.get::<_,String>(13)?,"error":r.get::<_,String>(14)?}))).map_err(err)?.collect::<std::result::Result<Vec<_>,_>>().map_err(err)?;
 Ok(json!({"point_id":pid,"items":rows}))
}
impl AppCore{
 pub fn get_mastery(&self,filter:&str)->Result<Value>{let i=self.inner.lock().map_err(|_|"STATE_LOCK")?;list(i.library.as_ref().ok_or("NO_LIBRARY")?,filter)}
 pub fn point_history(&self,pid:&str)->Result<Value>{let i=self.inner.lock().map_err(|_|"STATE_LOCK")?;history(i.library.as_ref().ok_or("NO_LIBRARY")?,pid)}
 pub fn rebuild_mastery(&self)->Result<Value>{let mut i=self.inner.lock().map_err(|_|"STATE_LOCK")?;rebuild(i.library.as_mut().ok_or("NO_LIBRARY")?)}
}
