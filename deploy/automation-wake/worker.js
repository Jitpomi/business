// Independent operations monitor. Never invokes a tenant workflow, payment or write capability.
import {timingSafeEqual} from 'node:crypto';
const now=()=>Math.floor(Date.now()/1000);
const response=(value,status=200)=>new Response(JSON.stringify(value),{status,headers:{'content-type':'application/json','cache-control':'no-store'}});
function auth(request,secret){const a=Buffer.from(request.headers.get('authorization')||''),b=Buffer.from(`Bearer ${secret||''}`);return secret?.length>=32&&a.length===b.length&&timingSafeEqual(a,b);}
export function origin(value){const u=new URL(value);if(u.protocol!=='https:'||u.username||u.password||u.search||u.hash||u.pathname!=='/')throw Error('Expected a trusted HTTPS origin');return u;}
export async function probe(env){
 const base=origin(env.BUSINESS_URL);let failure;
 try{
  const live=await fetch(new URL('/health',base),{redirect:'manual',signal:AbortSignal.timeout(110000)});
  if(live.status!==200)return {issues:['host_unavailable']};
  const r=await fetch(new URL('/health/ready',base),{redirect:'manual',headers:{authorization:`Bearer ${env.OPERATIONS_READ_TOKEN}`},signal:AbortSignal.timeout(30000)});
  if(r.status!==200)return {issues:[r.status===401?'monitor_credentials_rejected':'dependencies_unavailable']};
  const text=await r.text();if(text.length>8192)throw Error('Invalid readiness body');const v=JSON.parse(text);
  if(v.version!==1||v.dependencies!=='ready'||!v.metrics)throw Error('Invalid readiness protocol');
  const keys=['failed_jobs','failed_runs','overdue_jobs','attention_runs','attention_invoices','stale_reconciliations'];
  if(keys.some(k=>!Number.isSafeInteger(v.metrics[k])||v.metrics[k]<0))throw Error('Invalid aggregate');
  failure={issues:keys.filter(k=>v.metrics[k]>0),metrics:Object.fromEntries(keys.map(k=>[k,v.metrics[k]]))};
 }catch{failure={issues:['probe_unavailable']};}
 return failure;
}
async function state(env,id){const row=await env.MONITOR.prepare('SELECT state FROM monitors WHERE id=?').bind(id).first();return row?JSON.parse(row.state):{failures:0,recoveries:0,incident:null,checked_at:0};}
export async function record(env,id,observation,time=now()){
 const s=await state(env,id);const issues=[...observation.issues].sort();const bad=issues.length>0;
 const next={...s,checked_at:time,issues,metrics:observation.metrics||{},failures:bad?s.failures+1:0,recoveries:bad?0:s.recoveries+1};
 let event;
 if(bad&&next.failures>=2&&!s.incident){next.incident=crypto.randomUUID();event={id:next.incident,kind:'OUTAGE',issues};}
 if(!bad&&next.recoveries>=2&&s.incident){event={id:s.incident+'-recovered',kind:'RECOVERED',issues:[]};next.incident=null;}
 if(observation.counter!==undefined&&(event?.kind==='OUTAGE'||(bad&&s.incident)))next.counter_ack=observation.counter;
 const statements=[env.MONITOR.prepare('INSERT INTO monitors(id,state) VALUES(?,?) ON CONFLICT(id) DO UPDATE SET state=excluded.state').bind(id,JSON.stringify(next))];
 if(event){const body=JSON.stringify({subject:`${id==='drill'?'[DRILL] ':''}JITPOMI development ${event.kind}`,text:`${id==='drill'?'This is a monitoring drill; no production outage or payment is implied.\n':''}Monitor: ${id}\nState: ${event.kind}\nObserved at: ${new Date(time*1000).toISOString()}\nChecks: ${event.issues.join(', ')||'healthy'}\nNo customer details or credentials are included. Review the protected operations status and retained journals. Do not retry uncertain payments.`});statements.push(env.MONITOR.prepare('INSERT INTO outbox(id,body,created) VALUES(?,?,?) ON CONFLICT(id) DO NOTHING').bind(event.id,body,time));}
 await env.MONITOR.batch(statements);return next;
}
export async function deliver(env,time=now()){
 origin('https://api.resend.com/');if(!env.ALERT_TO||!env.ALERT_FROM||!env.ALERT_TOKEN)throw Error('Alert delivery is not configured');
 const rows=(await env.MONITOR.prepare('SELECT * FROM outbox WHERE accepted IS NULL AND uncertain=0 ORDER BY created LIMIT 10').all()).results;
 let sent=0;
 for(const row of rows){
  // Provider idempotency lasts 24 hours. An older ambiguous attempt requires operator reconciliation.
  if(row.first_attempt!==null&&time-row.first_attempt>=23*3600){await env.MONITOR.prepare('UPDATE outbox SET uncertain=1 WHERE id=? AND accepted IS NULL').bind(row.id).run();continue;}
  await env.MONITOR.prepare('UPDATE outbox SET first_attempt=COALESCE(first_attempt,?) WHERE id=?').bind(time,row.id).run();
  const payload=JSON.parse(row.body);
  if(sent++)await new Promise(resolve=>setTimeout(resolve,600));
  try{const r=await fetch('https://api.resend.com/emails',{method:'POST',redirect:'manual',signal:AbortSignal.timeout(15000),headers:{authorization:`Bearer ${env.ALERT_TOKEN}`,'content-type':'application/json','idempotency-key':`jitpomi-monitor/${row.id}`},body:JSON.stringify({from:env.ALERT_FROM,to:[env.ALERT_TO],...payload})});if(!r.ok)throw Error('Delivery refused');const v=await r.json();if(typeof v.id!=='string'||v.id.length>200)throw Error('Invalid delivery acknowledgement');await env.MONITOR.prepare('UPDATE outbox SET accepted=? WHERE id=? AND accepted IS NULL').bind(v.id,row.id).run();}
  catch{console.error('Operations alert delivery pending; durable idempotency key retained');}
 }
}
async function locked(env,fn){
 if(!env.MONITOR||!env.OPERATIONS_READ_TOKEN)throw Error('Operations monitoring is not configured');
 const owner=crypto.randomUUID(),time=now();
 const lease=await env.MONITOR.prepare("INSERT INTO lease(id,owner,expires) VALUES('monitor',?,?) ON CONFLICT(id) DO UPDATE SET owner=excluded.owner,expires=excluded.expires WHERE lease.expires<? RETURNING owner").bind(owner,time+600,time).first();
 if(lease?.owner!==owner)return {busy:true};
 try{return await fn();}finally{await env.MONITOR.prepare("DELETE FROM lease WHERE id='monitor' AND owner=?").bind(owner).run();}
}
export async function run(env){return locked(env,async()=>{
 const v=await probe(env);
 await record(env,'availability',{issues:v.metrics?[]:v.issues});
 if(v.metrics)for(const [key,value] of Object.entries(v.metrics)){
  const counter=['failed_jobs','failed_runs'].includes(key);const previous=counter?await state(env,key):null;
  await record(env,key,{issues:value>(counter?(previous.counter_ack||0):0)?[key]:[],...(counter?{counter:value}:{})});
 }
 await env.MONITOR.prepare('INSERT INTO monitors(id,state) VALUES(?,?) ON CONFLICT(id) DO UPDATE SET state=excluded.state').bind('business',JSON.stringify({...v,checked_at:now()})).run();
 await deliver(env);return v;
});}
export default {
 async scheduled(_event,env,ctx){ctx.waitUntil(run(env));},
 async fetch(request,env){
  const path=new URL(request.url).pathname;
  if(path==='/status'&&request.method==='GET'){
   if(!auth(request,env.MONITOR_STATUS_TOKEN))return response({},401);
   const s=await state(env,'business');const pending=await env.MONITOR.prepare('SELECT count(*) AS count,min(created) AS oldest FROM outbox WHERE accepted IS NULL').first();return response({...s,pending_alerts:pending.count,oldest_pending:pending.oldest});
  }
  if(!auth(request,env.MONITOR_ADMIN_TOKEN))return response({},401);
  if(request.method!=='POST')return response({},405);
  if(path==='/check')return response(await run(env));
  if(path==='/drill/down'||path==='/drill/up')return response(await locked(env,async()=>{const issues=path.endsWith('/down')?['synthetic_outage']:[];const s=await record(env,'drill',{issues});await deliver(env);return s;}));
  return response({},404);
 }
};
