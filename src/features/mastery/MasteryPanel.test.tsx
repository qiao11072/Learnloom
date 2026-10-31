// @vitest-environment jsdom
import {cleanup,fireEvent,render,screen,waitFor} from '@testing-library/react';
import {afterEach,expect,test,vi} from 'vitest';
import {MasteryPanel} from './MasteryPanel';
import {command} from '../../ipc/client';
vi.mock('../../ipc/client',()=>({command:vi.fn(),errorText:String}));
afterEach(()=>{cleanup();vi.clearAllMocks();});
test('shows recent performance and failed history without treating failure as zero; wiki uses saved version',async()=>{
 const onPage=vi.fn();vi.mocked(command).mockImplementation(async(op)=>op==='get_mastery'?{items:[{point_id:'p',name:'密度',page_id:'wiki',page_title:'密度 Wiki',status:'weak',few_samples:true,performance:{effective_count:1,sample_count:1,recent_score:0.5,consecutive_zero:0,consecutive_full:0}}]} as never:{items:[{attempt_id:'a',prompt:'旧题',answer:'保留答案',status:'failed',score:null,submitted_at:'1793200000000',graded_at:null,reference:'公式',explanation:'除法顺序',error:'INVALID_OUTPUT',context:{page_id:'wiki',revision_id:'old'}}]} as never);
 render(<MasteryPanel onPage={onPage}/>);fireEvent.click(await screen.findByRole('button',{name:'密度'}));await screen.findByText('保留答案');expect(screen.getByText(/近期表现 50%/)).toBeTruthy();expect(screen.getByText(/未计分/)).toBeTruthy();expect(screen.queryByText(/0\/2 分/)).toBeNull();fireEvent.click(screen.getByText('参考答案与知识解析'));fireEvent.click(screen.getByRole('button',{name:'阅读出题时 Wiki 版本'}));expect(onPage).toHaveBeenCalledWith('wiki','old');expect(vi.mocked(command).mock.calls.every(([op])=>['get_mastery','get_point_history'].includes(op))).toBe(true);
 fireEvent.change(screen.getByLabelText('筛选知识点'),{target:{value:'unseen'}});await waitFor(()=>expect(command).toHaveBeenCalledWith('get_mastery',{filter:'unseen'}));
});
test('unmounted performance reads do not start model requests',async()=>{let resolve:(v:unknown)=>void=()=>{};vi.mocked(command).mockImplementation(()=>new Promise(r=>{resolve=r}) as never);const view=render(<MasteryPanel onPage={()=>{}}/>);view.unmount();resolve({items:[]});await Promise.resolve();expect(command).toHaveBeenCalledTimes(1);});
