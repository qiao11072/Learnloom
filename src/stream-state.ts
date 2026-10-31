export type MessageSnapshot = {body:string; references:unknown[]; status:string; run_id:string; sequence:number};
export type Delta = {run_id:string; sequence:number; kind:'delta'|'reset'; text?:string};
export function receive(snapshot:MessageSnapshot,event:Delta):{snapshot:MessageSnapshot; refetch:boolean} {
  if(event.sequence<=snapshot.sequence) return {snapshot,refetch:false};
  if(event.run_id!==snapshot.run_id || event.kind==='reset' || event.sequence!==snapshot.sequence+1) return {snapshot,refetch:true};
  return {snapshot:{...snapshot,body:snapshot.body+(event.text??''),sequence:event.sequence},refetch:false};
}
// Subscribe first; replace with the authoritative query, then apply buffered events.
export function reconcile(snapshot:MessageSnapshot,buffer:Delta[]) {
  let current=snapshot;
  for(const event of buffer) {const result=receive(current,event);if(result.refetch)return result;current=result.snapshot;}
  return {snapshot:current,refetch:false};
}
