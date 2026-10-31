import {describe,it,expect} from 'vitest';
import {Message,Update,applyUpdate,mergeMessages} from './state';
const m:Message={id:'a',conversation_id:'c',role:'assistant',ordinal:1,body:'已保存',references:[],status:'streaming',answer_kind:null,job_id:'j',run_id:'r1',sequence:5,created_at:0,error:''};
const event=(v:Partial<Update>={}):Update=>({kb_id:'k',conversation_id:'c',entity_id:'a',run_id:'r1',sequence:6,kind:'delta',payload:{text:'正文'},...v});
describe('durable stream reconciliation',()=>{
 it('applies contiguous deltas once and drops snapshot-covered events',()=>{const a=applyUpdate([m],event());expect(a.messages[0].body).toBe('已保存正文');expect(applyUpdate(a.messages,event()).messages).toEqual(a.messages);expect(applyUpdate([m],event({sequence:4})).resync).toBe(false);});
 it('requests snapshots for missing events, unknown messages and new runs',()=>{for(const e of [event({sequence:8}),event({run_id:'r2'}),event({entity_id:'b'}),event({kind:'reset'}),event({kind:'resync'})])expect(applyUpdate([m],e).resync).toBe(true);});
 it('does not let delayed snapshots overwrite a retry or final response',()=>{const newer={...m,run_id:'r2',sequence:10,body:'new'};expect(mergeMessages([newer],[m])).toEqual([newer]);});
 it('only a canonical replacement exposes final references',()=>{const final={...m,sequence:6,status:'succeeded',references:[{evidence_id:'E1',page_id:'p',revision_id:'v',title:'来源',anchor:'chunk-0',excerpt:'原文',evidence_hash:'hash'}]};const got=applyUpdate([m],event({kind:'replace',payload:final}));expect(got.messages[0].references[0].revision_id).toBe('v');expect(got.resync).toBe(false);});
 it('rejects malformed replacements and deltas after completion',()=>{expect(applyUpdate([m],event({kind:'replace',payload:{...m,sequence:1}})).resync).toBe(true);expect(applyUpdate([{...m,status:'succeeded'}],event()).resync).toBe(true);});
});
