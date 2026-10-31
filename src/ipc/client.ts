import { invoke } from '@tauri-apps/api/core';
export type Status='saved'|'queued'|'running'|'succeeded'|'failed'|'cancelled'|'interrupted';
export interface ModelSettings {config_revision:number;preferences_revision:number;endpoint:string;model:string;context:number;language:'zh-CN'|'en';has_key:boolean;check_state:'untested'|'testing'|'passed'|'failed';model_ready:boolean}
export interface Location {token:string;path:string;cancelled?:boolean}
export interface Source {id:string;title:string;created_at:number;status:Status;job_id:string;run_id:string;stage:string;error:string}
export interface PageSummary {id:string;title:string;revision:string;index_state:string;scope:string}
export interface Bootstrap {setup:{step:number;deferred:boolean;completed:boolean};settings:ModelSettings;library:{id:string;name:string;path:string}|null;default_location:Location;sources:Source[];pages:PageSummary[];warning:string;recovered_jobs:number;sequence:number}
export interface Page {id:string;title:string;body:string;revision:string;current_revision:string;index_state:string;sources:{id:string;title:string}[];links:{id:string;title:string}[];points:{id:string;name:string;evidence:string}[]}
export interface SourceContent {id:string;title:string;body:string;hash:string}
export interface Failure {code:string;message:string;retryable:boolean;request_id:string}
export async function command<T>(operation:string,args:Record<string,unknown>={}):Promise<T>{return invoke<T>('learnloom',{operation,args});}
const messages:Record<string,string>={INDEX_ERROR:'搜索索引异常，请重新打开知识库恢复。',INDEX_UNAVAILABLE:'相关页面索引尚不可用，请恢复后重试。',STREAM_INTERRUPTED:'回答连接中断，已保存部分正文；请显式重试。',INVALID_CITATION:'模型引用未通过来源校验，请重试。',BUSY:'当前对话仍有回答在进行，请完成或取消后继续。',INVALID_REQUEST:'服务拒绝请求，请检查模型ID和结构化输出支持。',PAGE_CORRUPTED:'页面文件损坏，请从备份恢复；原始资料仍保留。',SOURCE_CORRUPTED:'原始资料校验失败，请保留文件并重新导入。',AUTH_FAILED:'认证失败，请检查API Key。',MODEL_UNAVAILABLE:'模型不可用，请检查模型ID。',NETWORK_ERROR:'无法连接云端服务，请检查服务地址和网络。',TIMEOUT:'请求超时，可稍后重试。',RATE_LIMITED:'服务请求过于频繁，请稍后重试。',PROVIDER_ERROR:'云端服务暂时不可用。',MODEL_NOT_READY:'请先保存云端模型配置，并通过连接测试。',CREDENTIAL_UNAVAILABLE:'系统中未找到这版模型的凭据，请重新配置后显式重试。',INVALID_OUTPUT:'模型返回的内容未通过校验，资料已保留，可重试。',SOURCE_TOO_LARGE:'资料超出当前处理上限，请拆分后重试；原文仍已保存。',CONTEXT_BUDGET_INSUFFICIENT:'模型上下文设置太小，无法容纳必要指令。请调整设置后重试。',INPUT_TOO_LONG:'必要输入超出上下文预算，无法安全处理。',KB_ALREADY_OPEN:'这个知识库已被另一个窗口或实例占用。',CONFLICT:'内容或配置已变化，请刷新后重试。',NO_VALID_KNOWLEDGE:'未从资料得到可验证的知识内容，原文已保留。'};
export function errorText(error:unknown):string{if(typeof error==='object'&&error&&'code' in error){const e=error as Failure;return messages[e.code]??e.message;}return String(error);}
export const statusText:Record<Status,string>={saved:'待整理',queued:'等待整理',running:'正在整理',succeeded:'已整理',failed:'整理失败',cancelled:'已取消',interrupted:'上次运行中断'};
export function requestKey(){return crypto.randomUUID();}
export function displayPath(path:string){return path.startsWith('\\\\?\\')?path.slice(4):path;}
