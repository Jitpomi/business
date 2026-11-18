// Independent GitHub-run heartbeat observer. No tenant/data/payment credentials.
import {pathToFileURL} from 'node:url';
import {origin} from './worker.js';
export async function watchdog(env,request=fetch,time=Math.floor(Date.now()/1000)){
 const base=origin(env.MONITOR_URL);let reason;
 try{const r=await request(new URL('/status',base),{redirect:'manual',headers:{authorization:`Bearer ${env.MONITOR_STATUS_TOKEN}`},signal:AbortSignal.timeout(20000)});if(r.status!==200)throw Error();const s=await r.json();if(!Number.isSafeInteger(s.checked_at)||time-s.checked_at>900||s.checked_at>time+60)reason='heartbeat_stale';else if(s.pending_alerts>0&&(!s.oldest_pending||time-s.oldest_pending>900))reason='alert_delivery_stalled';else if(s.backup?.stale===true)reason='backup_stale';}
 catch{reason='monitor_unreachable';}
 if(env.WATCHDOG_DRILL==='true')reason='synthetic_drill';
 if(!reason)return {healthy:true};
 const window=Math.floor(time/3600);const drill=reason==='synthetic_drill';
 const r=await request('https://api.resend.com/emails',{method:'POST',redirect:'manual',signal:AbortSignal.timeout(15000),headers:{authorization:`Bearer ${env.ALERT_TOKEN}`,'content-type':'application/json','idempotency-key':`jitpomi-watchdog/${window}/${reason}`},body:JSON.stringify({from:env.ALERT_FROM,to:[env.ALERT_TO],subject:`${drill?'[DRILL] ':''}JITPOMI monitor watchdog`,text:`${drill?'This is an alert delivery drill. No outage or payment is implied.\n':''}The independent monitor check reports: ${reason}.\nWindow: ${new Date(window*3600000).toISOString()}\nInspect the monitor and durable notification journal. No customer data or credentials are included. Do not retry uncertain payments.`})});
 if(!r.ok)throw Error('Independent alert delivery was not acknowledged');const v=await r.json();if(!v.id)throw Error('Invalid alert acknowledgement');return {healthy:false,reason,accepted:v.id};
}
if(process.argv[1]&&import.meta.url===pathToFileURL(process.argv[1]).href){try{const v=await watchdog(process.env);console.log(JSON.stringify(v));if(!v.healthy&&process.env.WATCHDOG_DRILL!=='true')process.exitCode=1;}catch{console.error('Operations watchdog failed; check its configuration and alert provider.');process.exitCode=1;}}
