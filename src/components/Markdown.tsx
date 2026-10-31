import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import {command} from '../ipc/client';
export function Markdown({body,onPage,onSource}:{body:string;onPage?:(id:string)=>void;onSource?:(id:string)=>void}){
 return <div className="markdown"><ReactMarkdown remarkPlugins={[remarkGfm]} skipHtml components={{img:({alt})=><span className="image-placeholder">[图片：{alt??'远程图片未自动加载'}]</span>,a:({href,children})=>{
  if(onPage&&href?.startsWith('#page/'))return <button className="link" onClick={()=>onPage(href.slice(6))}>{children}</button>;
  if(onSource&&href?.startsWith('#source/'))return <button className="link" onClick={()=>onSource(href.slice(8))}>{children}</button>;
  if(href?.startsWith('#'))return <a href={href}>{children}</a>;
  if(href?.startsWith('https://'))return <button className="link" onClick={()=>void command('open_external_url',{url:href}).catch(()=>{})}>{children} ↗</button>;
  return <span>{children}</span>;
 }}}>{body}</ReactMarkdown></div>;
}
