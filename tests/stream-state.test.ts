import {describe,it,expect} from 'vitest';
import {reconcile,receive,type MessageSnapshot} from '../src/stream-state';
const snapshot:MessageSnapshot={body:'已保存',references:[{page_id:'p',revision_id:'r'}],status:'streaming',run_id:'r1',sequence:3};
describe('M0 snapshot/event convergence (T09)',()=>{
  it('drops duplicate and already included events received around the query',()=>{
    const result=reconcile(snapshot,[{run_id:'r1',sequence:2,kind:'delta',text:'旧'},{run_id:'r1',sequence:3,kind:'delta',text:'重复'},{run_id:'r1',sequence:4,kind:'delta',text:'正文'},{run_id:'r1',sequence:4,kind:'delta',text:'重复'}]);
    expect(result.refetch).toBe(false);expect(result.snapshot.body).toBe('已保存正文');expect(result.snapshot.references).toEqual(snapshot.references);
  });
  it.each([5,8])('refetches on a missing/out-of-order sequence %i',sequence=>{
    expect(receive(snapshot,{run_id:'r1',sequence,kind:'delta',text:'bad'})).toEqual({snapshot,refetch:true});
  });
  it('refetches new runs and resets rather than mixing bodies',()=>{
    expect(receive(snapshot,{run_id:'r2',sequence:4,kind:'delta',text:'bad'}).refetch).toBe(true);
    expect(receive(snapshot,{run_id:'r1',sequence:4,kind:'reset'}).refetch).toBe(true);
  });
  it('converges after refetch and ignores delayed old-run events',()=>{
    const fresh={...snapshot,body:'新答案',run_id:'r2',sequence:8};
    expect(reconcile(fresh,[{run_id:'r1',sequence:4,kind:'delta',text:'旧'},{run_id:'r2',sequence:9,kind:'delta',text:'完成'}]).snapshot.body).toBe('新答案完成');
  });
});
