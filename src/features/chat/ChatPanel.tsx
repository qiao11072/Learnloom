import {Markdown} from '../../components/Markdown';
import {useEffect,useRef,useState} from 'react';
import {Channel,invoke} from '@tauri-apps/api/core';
import {command,errorText,requestKey} from '../../ipc/client';
import {Conversation,Message,Reference,Update,applyUpdate,mergeMessages} from './state';
interface Summary {id:string;title:string;active_message_id:string|null}
const labels:Record<string,string>={queued:'等待回答',streaming:'正在生成 · 正文尚未完成校验',succeeded:'回答完成',failed:'回答失败 · 已保留部分正文',interrupted:'运行中断 · 可显式重试',cancelled:'已取消 · 可显式重试'};
export function ChatPanel({kbId,ready,onReference}:{kbId:string;ready:boolean;onReference:(r:Reference)=>void}){
 const key=`learnloom.chat.${kbId}`;
 const [cid,setCid]=useState<string|null>(()=>localStorage.getItem(key)),[draft,setDraft]=useState(()=>localStorage.getItem(`${key}.draft`)??''),[list,setList]=useState<Summary[]>([]),[listCursor,setListCursor]=useState<string|null>(null),[messages,setMessages]=useState<Message[]>([]),[older,setOlder]=useState<number|null>(null),[error,setError]=useState(''),[busy,setBusy]=useState(false),[loaded,setLoaded]=useState(false);
 const generation=useRef(0),reload=useRef<()=>Promise<void>>(async()=>{}),state=useRef<Message[]>([]),sendKey=useRef<{text:string;cid:string|null;key:string}|null>(null);
 function put(next:Message[]){state.current=next;setMessages(next);}
 async function loadList(cursor?:string){const r=await command<{items:Summary[];next_cursor:string|null}>('list_conversations',{cursor,limit:30});setList(old=>cursor?[...old,...r.items]:r.items);setListCursor(r.next_cursor);}
 useEffect(()=>{void loadList().catch(e=>setError(errorText(e)));},[kbId]);
 useEffect(()=>{localStorage.setItem(`${key}.draft`,draft);},[key,draft]);
 useEffect(()=>{
  const round=++generation.current;let active=true,sub:string|undefined,buffer:Update[]=[],snapshotReady=false,pending=false;
  put([]);setLoaded(!cid);setOlder(null);setError('');
  if(!cid){localStorage.removeItem(key);return()=>{active=false;};}
  localStorage.setItem(key,cid);
  const valid=()=>active&&generation.current===round;
  async function sync(){if(pending||!valid())return;pending=true;try{const s=await command<Conversation>('get_conversation',{conversation_id:cid,limit:30});if(!valid())return;put(mergeMessages(state.current,s.messages));if(!snapshotReady)setOlder(s.next_cursor);setLoaded(true);snapshotReady=true;const events=buffer;buffer=[];for(const e of events)receive(e);}catch(e){if(valid())setError(errorText(e));}finally{pending=false;}}
  function receive(e:Update){if(!valid()||e.kb_id!==kbId||e.conversation_id!==cid)return;if(!snapshotReady){buffer.push(e);return;}const result=applyUpdate(state.current,e);put(result.messages);if(result.resync)void sync();}
  reload.current=sync;
  void (async()=>{try{const channel=new Channel<Update>();channel.onmessage=receive;const s=await invoke<{subscription_id:string}>('subscribe_updates',{conversation_id:cid,channel});sub=s.subscription_id;if(!valid()){await invoke('unsubscribe_updates',{subscription_id:sub});return;}}catch(e){if(valid())setError(`实时连接不可用，正在通过已保存快照恢复：${errorText(e)}`);}if(valid())await sync();})();
  // Read-only polling also recovers a lost terminal event. It never starts a model request.
  const poll=setInterval(()=>{if(valid())void sync();},1500);
  return()=>{active=false;clearInterval(poll);if(sub)void invoke('unsubscribe_updates',{subscription_id:sub}).catch(()=>{});};
 },[cid,kbId]);
 const scrollPane=useRef<HTMLDivElement>(null),composer=useRef<HTMLTextAreaElement>(null),followBottom=useRef(true);
 useEffect(()=>{followBottom.current=true;},[cid]);
 useEffect(()=>{if(followBottom.current&&scrollPane.current)scrollPane.current.scrollTop=messages.length?scrollPane.current.scrollHeight:0;},[messages]);
 const running=messages.find(m=>m.role==='assistant'&&['queued','streaming'].includes(m.status));
 async function send(){const text=draft.trim();if(!text||busy||running)return;const round=generation.current;setBusy(true);setError('');if(!sendKey.current||sendKey.current.text!==text||sendKey.current.cid!==cid)sendKey.current={text,cid,key:requestKey()};try{const r=await command<{conversation_id:string}>('ask_knowledge',{conversation_id:cid,text,request_key:sendKey.current.key});if(round!==generation.current)return;setDraft('');sendKey.current=null;if(cid===r.conversation_id)await reload.current();else setCid(r.conversation_id);await loadList();}catch(e){if(round===generation.current)setError(errorText(e));}finally{setBusy(false);}}
 async function control(m:Message,retry:boolean){const round=generation.current;setBusy(true);try{await command(retry?'retry_job':'cancel_job',{job_id:m.job_id,expected_run_id:m.run_id,request_key:requestKey()});if(round===generation.current)await reload.current();}catch(e){if(round===generation.current)setError(errorText(e));}finally{setBusy(false);}}
 async function previous(){if(older===null)return;const round=generation.current;try{const s=await command<Conversation>('get_conversation',{conversation_id:cid,message_cursor:older,limit:30});if(round!==generation.current)return;put(mergeMessages(state.current,s.messages));setOlder(s.next_cursor);}catch(e){setError(errorText(e));}}
 return <div className="chat-layout">
  <aside className="chat-history"><div className="chat-history-heading"><h2>对话</h2><button className="chat-new" aria-label="新对话" disabled={busy} onClick={()=>{setCid(null);sendKey.current=null;}}>＋ 新对话</button></div>
   <div className="chat-history-list">{list.map(c=><button className={`chat-history-item ${cid===c.id?'selected':''}`} key={c.id} onClick={()=>setCid(c.id)}><span aria-hidden="true">◌</span><span>{c.title}</span></button>)}{!list.length&&<p className="hint">你的思考，从这里留下记录。</p>}{listCursor&&<button className="link" onClick={()=>void loadList(listCursor).catch(e=>setError(errorText(e)))}>更多对话</button>}</div>
   <p className="chat-history-foot">对话自动保存在当前知识库</p>
  </aside>
  <section className="chat-thread" aria-label="学习对话">
   <header className="chat-thread-header"><div><h2>学习对话</h2><p>结合你的知识库，一起理解、比较与延伸。</p></div><span className="chat-mode">{ready?'可以开始提问':'等待模型配置'}</span></header>
   {error&&<div className="warning" role="alert">{error}</div>}
   <div className="chat-messages" ref={scrollPane} onScroll={()=>{const el=scrollPane.current;if(el)followBottom.current=el.scrollHeight-el.scrollTop-el.clientHeight<100;}}>
    {!loaded&&<p role="status">正在加载已保存会话…</p>}{older!==null&&<button className="link" onClick={()=>void previous()}>加载更早消息</button>}
    {loaded&&!messages.length&&<div className="chat-welcome"><span className="chat-welcome-symbol" aria-hidden="true">✦</span><p className="chat-eyebrow">从一个好问题开始</p><h3>今天，想弄懂什么？</h3><p>从资料中的细节，到知识之间的联系。<br/>资料不足时，AI 可以补充说明，并单独标注。</p><div className="chat-starters">{['用一个例子解释这个概念','帮我比较两个容易混淆的概念','这个知识可以怎样用在实践中？'].map(s=><button key={s} className="secondary" onClick={()=>{setDraft(s);composer.current?.focus();}}>{s}<span aria-hidden="true">↗</span></button>)}</div></div>}
    {messages.map(m=><article className={`chat-message ${m.role}`} key={m.id}>
     <div className="chat-message-meta"><span className="chat-avatar" aria-hidden="true">{m.role==='user'?'你':'✦'}</span><strong>{m.role==='user'?'你':'Learnloom'}</strong>{m.role==='assistant'&&<small role="status">{m.status==='succeeded'?(m.answer_kind==='supplemented'?'含 AI 补充 · 非知识库依据':m.answer_kind==='insufficient_context'?'现有信息不足':'依据知识库回答'):labels[m.status]}</small>}</div>
     <div className="chat-body">{m.body?<Markdown body={m.body}/>:<p className="chat-pending">正在思考你的问题…</p>}</div>
     {m.error&&<p className="warning">{errorText({code:m.error.split(':')[0],message:m.error})}</p>}
     {m.references.length>0&&<details className="chat-references" aria-label="回答来源"><summary>知识库来源 · {m.references.length} 处</summary><div>{m.references.map(r=><button className="secondary" key={r.evidence_id} onClick={()=>onReference(r)}>[{r.evidence_id}] {r.title}<small>{r.excerpt}</small></button>)}</div></details>}
     {m.role==='assistant'&&['queued','streaming'].includes(m.status)&&<button className="ghost" disabled={busy} onClick={()=>void control(m,false)}>取消回答</button>}{m.role==='assistant'&&['failed','interrupted','cancelled'].includes(m.status)&&<button disabled={busy||!ready||!!running} onClick={()=>void control(m,true)}>重试回答</button>}
    </article>)}
   </div>
   <form className="chat-composer" onSubmit={e=>{e.preventDefault();void send();}}><label className="sr-only" htmlFor="chat-question">问题或追问</label><textarea id="chat-question" ref={composer} aria-label="问题或追问" value={draft} maxLength={16000} onChange={e=>setDraft(e.target.value)} onKeyDown={e=>{if(e.key==='Enter'&&(e.ctrlKey||e.metaKey)&&!e.nativeEvent.isComposing){e.preventDefault();if(ready&&loaded)void send();}}} rows={2} placeholder="写下问题，或继续追问…"/>
    <div className="chat-compose-actions"><span>{running?'正在回答，完成或取消后可继续':!ready?'请先在基础设置完成模型连接测试':'Enter 换行 · Ctrl + Enter 发送'}</span><button disabled={busy||!ready||!!running||!draft.trim()||!loaded} type="submit">{busy?'正在保存…':'发送问题'} <span aria-hidden="true">↑</span></button></div>
    <p className="chat-disclaimer">知识库引用可追溯；AI 补充来自模型知识，未经联网核验。</p>
   </form>
  </section>
 </div>;
}
