// @vitest-environment jsdom
import {cleanup,fireEvent,render,screen,waitFor} from '@testing-library/react';
import {afterEach,expect,test,vi} from 'vitest';
import {ReviewPanel} from './ReviewPanel';
import {command} from '../../ipc/client';
vi.mock('../../ipc/client',()=>({command:vi.fn(),errorText:String,requestKey:()=> 'request'}));
afterEach(()=>{cleanup();vi.clearAllMocks();});
const q={id:'q',point_id:'p',prompt:'解释催化作用',draft:'保存的草稿',draft_revision:2,reused_from:null};
const session={id:'s',date:'2026-10-28',status:'ready',actual_count:1,target_count:5,job_id:'j',run_id:'r',sequence:1,error:'',reasons:[],questions:[q]};
test('opening history restores draft without generating or exposing reference; submit uses draft before immutable answer',async()=>{
 let submitted=false;
 vi.mocked(command).mockImplementation(async(op,args)=>{
  if(op==='list_review_sessions')return {items:[session]} as never;
  if(op==='get_review_session')return {...session,questions:[submitted?{...q,reference:'参考内容',explanation:'解析内容',attempt:{id:'a',answer:'新答案',submitted_at:'001',status:'failed',score:null,feedback:null,job_id:'g',run_id:'r1',error:'INVALID_OUTPUT'}}:q]} as never;
  if(op==='save_answer_draft')return {revision:3} as never;
  if(op==='submit_answer'){expect(args?.answer).toBe('新答案');submitted=true;return {} as never;}
  return {} as never;
 });
 render(<ReviewPanel ready onPage={()=>{}}/>);
 fireEvent.click(await screen.findByRole('button',{name:/2026-10-28/}));
 const input=await screen.findByLabelText('第1题答案');expect((input as HTMLTextAreaElement).value).toBe('保存的草稿');expect(screen.queryByText('参考内容')).toBeNull();
 expect(vi.mocked(command).mock.calls.some(([op])=>['get_daily_session','submit_answer','retry_job'].includes(op))).toBe(false);
 fireEvent.change(input,{target:{value:'新答案'}});fireEvent.click(screen.getByRole('button',{name:'提交答案'}));
 await screen.findByRole('button',{name:'用原答案重试批改'});expect(screen.getByText('新答案')).toBeTruthy();expect(screen.getByText(/本次不计有效表现/)).toBeTruthy();expect(screen.queryByText(/0 \/ 2 分/)).toBeNull();
 fireEvent.click(screen.getByRole('button',{name:'用原答案重试批改'}));await waitFor(()=>expect(command).toHaveBeenCalledWith('retry_job',expect.objectContaining({job_id:'g',expected_run_id:'r1'})));
});
test('unmount ignores late history and never schedules paid work',async()=>{
 let resolve:(v:unknown)=>void=()=>{};vi.mocked(command).mockImplementation(async(op)=>op==='list_review_sessions'?new Promise(r=>{resolve=r}) as never:{} as never);
 const view=render(<ReviewPanel ready onPage={()=>{}}/>);view.unmount();resolve({items:[session]});await Promise.resolve();expect(vi.mocked(command).mock.calls.every(([op])=>op==='list_review_sessions')).toBe(true);
});
