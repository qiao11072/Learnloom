//! Whole-library snapshots: immutable files plus SQLite's online backup API.
use crate::{hash,knowledge::{self,Library},Result};
use serde::{Serialize,Deserialize};
use serde_json::{json,Value};
use std::{fs,path::{Path,PathBuf}};
fn err(e:impl std::fmt::Display)->String{e.to_string()}
#[derive(Serialize,Deserialize)]
struct Entry{path:String,sha256:String}
#[derive(Serialize,Deserialize)]
struct Manifest{format:u32,library_id:String,created_at:i64,files:Vec<Entry>}
fn digest(path:&Path)->Result<String>{use sha2::{Digest,Sha256};Ok(format!("{:x}",Sha256::digest(fs::read(path).map_err(err)?)))}
fn empty_target(source:&Path,target:&Path)->Result<PathBuf>{
 let target=knowledge::normalize_location(target)?;
 if target.starts_with(source){return Err("UNSAFE_BACKUP_TARGET: 目标不能位于源知识库内".into())}
 if target.exists()&&(!target.is_dir()||fs::read_dir(&target).map_err(err)?.next().is_some()){return Err("EXISTING_CONTENT: 备份或恢复目标必须为空目录".into())}
 fs::create_dir_all(&target).map_err(err)?;Ok(target)
}
fn walk(root:&Path,dir:&Path,files:&mut Vec<String>)->Result<()>{
 for entry in fs::read_dir(dir).map_err(err)?{let path=entry.map_err(err)?.path();knowledge::no_reparse(&path)?;
  let rel=path.strip_prefix(root).map_err(err)?.to_string_lossy().replace('\\',"/");
  if ["learnloom.json","backup-manifest.json",".learnloom/write.lock",".learnloom/learnloom.sqlite3",".learnloom/learnloom.sqlite3-wal",".learnloom/learnloom.sqlite3-shm"].contains(&rel.as_str()){continue}
  if path.is_dir(){walk(root,&path,files)?}else if path.is_file(){files.push(rel)}
 }Ok(())
}
pub fn create(lib:&Library,target:&Path)->Result<Value>{
 let busy:bool=lib.db.conn.query_row("SELECT EXISTS(SELECT 1 FROM jobs WHERE status IN('queued','running'))",[],|r|r.get(0)).map_err(err)?;
 if busy{return Err("BUSY: 请先完成或取消后台任务再备份".into())}
 let target=empty_target(&lib.root,target)?;let mut paths=vec![];walk(&lib.root,&lib.root,&mut paths)?;paths.sort();let mut entries=vec![];
 for rel in paths{let src=knowledge::safe_file(&lib.root,&rel)?;let dst=knowledge::safe_file(&target,&rel)?;fs::create_dir_all(dst.parent().ok_or("INVALID_PATH")?).map_err(err)?;fs::copy(&src,&dst).map_err(err)?;entries.push(Entry{path:rel,sha256:digest(&dst)?});}
 fs::create_dir_all(target.join(".learnloom")).map_err(err)?;
 let db=knowledge::safe_file(&target,".learnloom/learnloom.sqlite3")?;
 lib.db.conn.backup("main",&db,None).map_err(err)?;
 // Freeze the snapshot as a self-contained rollback-journal database before hashing.
 // Opening a WAL-mode snapshot even read-only could otherwise create sidecars.
 {let frozen=rusqlite::Connection::open(&db).map_err(err)?;frozen.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA journal_mode=DELETE;").map_err(err)?;}
 entries.push(Entry{path:".learnloom/learnloom.sqlite3".into(),sha256:digest(&db)?});
 let marker=fs::read(knowledge::safe_file(&lib.root,"learnloom.json")?).map_err(err)?;
 entries.push(Entry{path:"learnloom.json".into(),sha256:hash(std::str::from_utf8(&marker).map_err(err)?)});
 let manifest=Manifest{format:1,library_id:lib.marker.id.clone(),created_at:knowledge::now(),files:entries};
 knowledge::atomic_write(&target.join("backup-manifest.json"),&serde_json::to_vec_pretty(&manifest).map_err(err)?)?;
 // Marker is the publication boundary: partial snapshots cannot open as libraries.
 knowledge::atomic_write(&target.join("learnloom.json"),&marker)?;
 verify(&target)?;Ok(json!({"path":target,"library_id":lib.marker.id,"files":manifest.files.len(),"method":"sqlite_online_backup_and_whole_library_files","verified":true}))
}
pub fn verify(root:&Path)->Result<Value>{
 let root=fs::canonicalize(root).map_err(err)?;knowledge::no_reparse(&root)?;
 let m:Manifest=serde_json::from_slice(&fs::read(knowledge::safe_file(&root,"backup-manifest.json")?).map_err(err)?).map_err(err)?;
 if m.format!=1{return Err("UNSUPPORTED_BACKUP".into())}
 for e in &m.files{let path=knowledge::safe_file(&root,&e.path)?;if digest(&path)?!=e.sha256{return Err(format!("BACKUP_CORRUPTED: {}",e.path))}}
 if !m.files.iter().any(|e|e.path==".learnloom/learnloom.sqlite3")||!m.files.iter().any(|e|e.path=="learnloom.json"){return Err("BACKUP_INCOMPLETE".into())}
 let db=rusqlite::Connection::open_with_flags(knowledge::safe_file(&root,".learnloom/learnloom.sqlite3")?,rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(err)?;
 let integrity:String=db.query_row("PRAGMA integrity_check",[],|r|r.get(0)).map_err(err)?;if integrity!="ok"{return Err("BACKUP_DATABASE_CORRUPTED".into())}
 let marker:Value=serde_json::from_slice(&fs::read(knowledge::safe_file(&root,"learnloom.json")?).map_err(err)?).map_err(err)?;
 if marker["id"].as_str()!=Some(m.library_id.as_str()){return Err("BACKUP_IDENTITY_MISMATCH".into())}
 for table in ["sources","revisions"]{let mut st=db.prepare(&format!("SELECT path,hash FROM {table}")).map_err(err)?;let required=st.query_map([],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?))).map_err(err)?;
  for item in required{let(path,expected)=item.map_err(err)?;if !m.files.iter().any(|e|e.path==path){return Err(format!("BACKUP_INCOMPLETE: {path}"))}let raw=fs::read_to_string(knowledge::safe_file(&root,&path)?).map_err(err)?;let content=if table=="revisions"{knowledge::body_only(&raw)}else{&raw};if hash(content)!=expected{return Err(format!("BACKUP_CONTENT_MISMATCH: {path}"))}}
 }
 let mut check=db.prepare("PRAGMA foreign_key_check").map_err(err)?;if check.query([]).map_err(err)?.next().map_err(err)?.is_some(){return Err("BACKUP_FOREIGN_KEY_ERROR".into())}
 Ok(json!({"verified":true,"library_id":m.library_id,"files":m.files.len(),"created_at":m.created_at}))
}
pub fn restore(source:&Path,target:&Path)->Result<Value>{
 let source=fs::canonicalize(source).map_err(err)?;verify(&source)?;
 let target=empty_target(&source,target)?;
 let manifest:Manifest=serde_json::from_slice(&fs::read(source.join("backup-manifest.json")).map_err(err)?).map_err(err)?;
 for e in &manifest.files{if e.path=="learnloom.json"{continue}let src=knowledge::safe_file(&source,&e.path)?;let dst=knowledge::safe_file(&target,&e.path)?;fs::create_dir_all(dst.parent().ok_or("INVALID_PATH")?).map_err(err)?;fs::copy(src,&dst).map_err(err)?;if digest(&dst)?!=e.sha256{return Err("BACKUP_CHANGED: 恢复期间源备份变化".into())}}
 fs::copy(source.join("backup-manifest.json"),target.join("backup-manifest.json")).map_err(err)?;
 knowledge::atomic_write(&target.join("learnloom.json"),&fs::read(source.join("learnloom.json")).map_err(err)?)?;verify(&target)?;
 Ok(json!({"restored":true,"path":target,"library_id":manifest.library_id,"files":manifest.files.len()}))
}
