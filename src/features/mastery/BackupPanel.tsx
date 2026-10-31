import {useState} from 'react';
import {command,errorText,Location,displayPath} from '../../ipc/client';
export function BackupPanel(){
 const [busy,setBusy]=useState(false),[message,setMessage]=useState('');
 async function choose(){const r=await command<Location>('choose_backup_directory');return r.cancelled?null:r;}
 async function backup(){setBusy(true);setMessage('');try{const target=await choose();if(target){const r=await command<{files:number}>('backup_library',{token:target.token});setMessage(`整库备份已校验：${displayPath(target.path)}（${r.files} 个文件）`);}}catch(e){setMessage(errorText(e));}finally{setBusy(false);}}
 async function restore(){setBusy(true);setMessage('先选择已有备份目录，再选择空的恢复目录。');try{const source=await choose();if(!source)return;setMessage('现在选择空的恢复目录。');const target=await choose();if(target){await command('restore_library_backup',{source_token:source.token,target_token:target.token});setMessage(`整库恢复并校验完成：${displayPath(target.path)}。请在知识库位置中选择该目录打开。`);}}catch(e){setMessage(errorText(e));}finally{setBusy(false);}}
 return <section className="card"><h2>整库备份与恢复</h2><p>备份包含原始资料、Wiki 所有版本、学习记录和数据库，使用 SQLite 在线备份并逐文件校验。请先完成或取消后台任务，选择知识库以外的空目录。</p><p className="hint">云端密钥仍保存在本机系统凭据库。恢复到其他电脑后需要重新配置模型。备份副本请保持不变，恢复到另一个空目录再打开。</p><div className="actions"><button disabled={busy} onClick={()=>void backup()}>备份整个知识库</button><button disabled={busy} className="secondary" onClick={()=>void restore()}>从备份恢复到空目录</button></div>{message&&<p role="status">{message}</p>}</section>;
}
