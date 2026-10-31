export interface Reference {evidence_id:string;page_id:string;revision_id:string;title:string;anchor:string;excerpt:string;evidence_hash:string}
export interface Message {id:string;conversation_id:string;role:'user'|'assistant';ordinal:number;body:string;references:Reference[];status:string;answer_kind:string|null;job_id:string|null;run_id:string|null;sequence:number;created_at:number;error:string}
export interface Update {kb_id:string;conversation_id:string;entity_id:string;run_id:string|null;sequence:number;kind:string;payload:unknown}
export interface Conversation {conversation:{id:string;title:string;active_message_id:string|null};messages:Message[];next_cursor:number|null}
// Snapshots are canonical. In particular, a delayed snapshot cannot roll back a retry.
export function mergeMessages(current:Message[],incoming:Message[]):Message[]{
 const map=new Map(current.map(m=>[m.id,m]));
 for(const m of incoming){const old=map.get(m.id);if(!old||m.sequence>=old.sequence)map.set(m.id,m);}
 return [...map.values()].sort((a,b)=>a.ordinal-b.ordinal);
}
export function applyUpdate(messages:Message[],event:Update):{messages:Message[];resync:boolean}{
 const old=messages.find(m=>m.id===event.entity_id);
 if(event.kind==='resync')return {messages,resync:true};
 if(old&&event.sequence<=old.sequence)return {messages,resync:false};
 if(!old||old.run_id!==event.run_id||event.kind==='reset'||event.sequence!==old.sequence+1)return {messages,resync:true};
 if(event.kind==='delta'){
  if(old.status!=='streaming')return {messages,resync:true};
  const text=(event.payload as {text?:unknown}).text;
  if(typeof text!=='string')return {messages,resync:true};
  return {messages:mergeMessages(messages,[{...old,body:old.body+text,sequence:event.sequence}]),resync:false};
 }
 const m=event.payload as Message;
 if(m.id!==old.id||m.sequence!==event.sequence||m.run_id!==event.run_id)return {messages,resync:true};
 return {messages:mergeMessages(messages,[m]),resync:false};
}
