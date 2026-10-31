import {useEffect,useState} from 'react';
import {Bootstrap,ModelSettings,command,errorText} from '../../ipc/client';
function Spinner(){return <span className="spinner" aria-hidden="true"/>;}
export function ModelForm({settings,refresh,notify}:{settings:ModelSettings;refresh:()=>Promise<void>;notify:(s:string)=>void}){
 const [endpoint,setEndpoint]=useState(settings.endpoint),[model,setModel]=useState(settings.model),[key,setKey]=useState(''),[context,setContext]=useState(settings.context),[busy,setBusy]=useState(false),[testing,setTesting]=useState(false);
 useEffect(()=>{setEndpoint(settings.endpoint);setModel(settings.model);setContext(settings.context);setKey('');},[settings.config_revision,settings.endpoint,settings.model,settings.context]);
 const dirty=endpoint!==settings.endpoint||model!==settings.model||key!==''||context!==settings.context;
 const contextError=!Number.isInteger(context)||context<2048||context>1048576?'上下文上限需为 2048～1048576 的整数。未知时请填 4096。':'';
 const invalid=!!contextError||!model.trim()||!endpoint.startsWith('https://');
 async function persist(){const saved=await command<Bootstrap>('save_model_config',{endpoint,model,key,context,expected_config_revision:settings.config_revision});setKey('');return saved.settings.config_revision;}
 async function save(){if(busy||testing||invalid)return;setBusy(true);try{await persist();await refresh();notify('模型配置已保存，可以测试连接。');}catch(e){notify(errorText(e));}finally{setBusy(false);}}
 async function check(){if(busy||testing||invalid)return;setBusy(true);try{const revision=dirty?await persist():settings.config_revision;setTesting(true);const r=await command<{passed:boolean;applied:boolean;error:string}>('test_model_connection',{config_revision:revision});await refresh();notify(!r.applied?'设置已变化，旧连接结果未应用。':r.passed?'连接通过。':errorText({code:r.error?.split(':')[0]??'INVALID_OUTPUT',message:r.error??'未获得有效响应'}));}catch(e){notify(errorText(e));}finally{setTesting(false);setBusy(false);}}
 async function language(value:string){try{await command('save_preferences',{language:value,expected_preferences_revision:settings.preferences_revision});await refresh();notify('生成语言已保存，将用于新的整理任务和显式重试。');}catch(e){notify(errorText(e));}}
 return <div className="settings-grid"><section className="card"><h2>云端模型</h2><p className="muted">整理时会将所需资料片段发送给此云端服务。</p>
  <label>服务预设<select disabled={busy||testing} aria-label="服务预设" value={endpoint==='https://api.deepseek.com'?'deepseek':'custom'} onChange={e=>{if(e.target.value==='deepseek'){setEndpoint('https://api.deepseek.com');setModel('deepseek-flash');}else{setEndpoint('https://');setModel('');}}}><option value="deepseek">DeepSeek</option><option value="custom">自定义云端 OpenAI 兼容服务</option></select></label>
  <label>HTTPS 服务地址<input disabled={busy||testing} aria-label="HTTPS 服务地址" value={endpoint} onChange={e=>setEndpoint(e.target.value)} placeholder="https://api.example.com/v1"/></label>
  <label>模型 ID<input disabled={busy||testing} aria-label="模型 ID" value={model} onChange={e=>setModel(e.target.value)} placeholder="填写服务实际使用的模型 ID"/></label>
  <label>API Key<input disabled={busy||testing} aria-label="API Key" type="password" autoComplete="off" value={key} onChange={e=>setKey(e.target.value)} placeholder={settings.has_key?'已保存；留空保留当前地址的凭据':'按服务要求填写'}/></label>
  <p className="hint">密钥保存在系统凭据库；更换服务地址不会沿用原地址的密钥。</p>
  <label>上下文上限<input disabled={busy||testing} aria-label="上下文上限" aria-invalid={!!contextError} aria-describedby="context-help" type="number" min={2048} max={1048576} step={1} value={context} onChange={e=>setContext(Number(e.target.value))}/></label><p id="context-help" className="hint">按模型实际支持的容量填写，支持至 1048576（1M）。4096 仅为未知时的默认值，不是上限。</p>{contextError&&<p className="warning" role="alert">{contextError}</p>}
  <div className="actions"><button onClick={()=>void save()} disabled={busy||testing||invalid}>保存模型</button><button className="secondary" onClick={()=>void check()} disabled={testing||busy||invalid}>{busy?<Spinner/>:null}{testing?'测试中…':busy?'正在保存…':dirty?'保存并测试连接':'测试连接'}</button>{testing&&<button className="ghost" onClick={()=>void command('cancel_connection_check').then(refresh).catch(e=>notify(errorText(e)))}>取消检查</button>}</div>
  <p className={`connection ${settings.check_state}`}>已保存配置版本 {settings.config_revision} · {({untested:'未测试',testing:'测试中',passed:'连接通过',failed:'连接失败'})[settings.check_state]}</p>{dirty&&<p className="hint">有未保存的更改；点击“保存并测试连接”会先保存，再测试当前填写的配置。</p>}
 </section><section className="card"><h2>生成偏好</h2><label>内容生成语言<select aria-label="内容生成语言" value={settings.language} onChange={e=>void language(e.target.value)}><option value="zh-CN">简体中文</option><option value="en">English</option></select></label><p className="muted">新任务使用保存后的语言。正在处理的任务保留启动时的配置；已有内容不会自动重写。</p></section></div>;
}
