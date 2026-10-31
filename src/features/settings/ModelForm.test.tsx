// @vitest-environment jsdom
import {cleanup,fireEvent,render,screen,waitFor} from '@testing-library/react';
import {afterEach,expect,test,vi} from 'vitest';
import {ModelForm} from './ModelForm';
import {command,ModelSettings} from '../../ipc/client';
vi.mock('../../ipc/client',()=>({command:vi.fn(),errorText:(e:{message:string})=>e.message}));
afterEach(()=>{cleanup();vi.resetAllMocks();});
const settings:ModelSettings={config_revision:1,preferences_revision:1,endpoint:'https://api.deepseek.com',model:'deepseek-flash',context:4096,language:'zh-CN',has_key:false,check_state:'untested',model_ready:false};
test('new key can be saved and tested in one action using the returned revision',async()=>{
 let finishSave:(v:unknown)=>void=()=>{};
 vi.mocked(command).mockImplementation((op)=>op==='save_model_config'?new Promise(r=>{finishSave=r;}) as never:Promise.resolve({passed:true,applied:true}) as never);
 const notify=vi.fn();render(<ModelForm settings={settings} refresh={vi.fn()} notify={notify}/>);
 fireEvent.change(screen.getByLabelText('API Key'),{target:{value:'test-only-key'}});
 fireEvent.click(screen.getByRole('button',{name:'保存并测试连接'}));
 expect(command).toHaveBeenCalledTimes(1);
 expect((screen.getByLabelText('API Key') as HTMLInputElement).disabled).toBe(true);
 finishSave({settings:{...settings,config_revision:8}});
 await waitFor(()=>expect(command).toHaveBeenCalledWith('test_model_connection',{config_revision:8}));
 await waitFor(()=>expect(notify).toHaveBeenCalledWith('连接通过。'));
});
test('failed save preserves the draft and never tests stale settings',async()=>{
 vi.mocked(command).mockRejectedValue({message:'系统凭据保存失败'});
 const notify=vi.fn();render(<ModelForm settings={settings} refresh={vi.fn()} notify={notify}/>);
 fireEvent.change(screen.getByLabelText('API Key'),{target:{value:'test-only-key'}});
 fireEvent.click(screen.getByRole('button',{name:'保存并测试连接'}));
 await waitFor(()=>expect(notify).toHaveBeenCalledWith('系统凭据保存失败'));
 expect(command).toHaveBeenCalledTimes(1);expect((screen.getByLabelText('API Key') as HTMLInputElement).value).toBe('test-only-key');
});
test('invalid context gets an inline range error; 1M enables saving and testing',()=>{
 render(<ModelForm settings={settings} refresh={vi.fn()} notify={vi.fn()}/>);
 fireEvent.change(screen.getByLabelText('上下文上限'),{target:{value:'1048577'}});
 expect(screen.getByRole('alert').textContent).toContain('2048～1048576');
 expect((screen.getByRole('button',{name:'保存并测试连接'}) as HTMLButtonElement).disabled).toBe(true);
 fireEvent.click(screen.getByRole('button',{name:'保存模型'}));expect(command).not.toHaveBeenCalled();
 fireEvent.change(screen.getByLabelText('上下文上限'),{target:{value:'1048576'}});
 expect(screen.queryByRole('alert')).toBeNull();expect((screen.getByRole('button',{name:'保存并测试连接'}) as HTMLButtonElement).disabled).toBe(false);
});
test('unchanged settings test directly without creating another configuration revision',async()=>{
 vi.mocked(command).mockResolvedValue({passed:false,applied:true,error:'AUTH_FAILED: rejected'});
 const notify=vi.fn();render(<ModelForm settings={settings} refresh={vi.fn()} notify={notify}/>);
 fireEvent.click(screen.getByRole('button',{name:'测试连接'}));
 await waitFor(()=>expect(notify).toHaveBeenCalledWith('AUTH_FAILED: rejected'));
 expect(command).toHaveBeenCalledTimes(1);expect(command).toHaveBeenCalledWith('test_model_connection',{config_revision:1});
});
