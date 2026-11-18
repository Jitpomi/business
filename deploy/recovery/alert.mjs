// Failure notices contain only a run URL; no exported data, environment values or credentials.
const run=process.env.GITHUB_RUN_ID;
if(!/^\d+$/.test(run||''))throw Error('Explicit backup run required');
const r=await fetch('https://api.resend.com/emails',{method:'POST',redirect:'manual',signal:AbortSignal.timeout(20000),headers:{authorization:`Bearer ${process.env.ALERT_TOKEN}`,'content-type':'application/json','idempotency-key':`jitpomi-backup-failure/${run}/${process.env.GITHUB_RUN_ATTEMPT}`},body:JSON.stringify({from:process.env.ALERT_FROM,to:[process.env.ALERT_TO],subject:'JITPOMI development backup failed',text:`A scheduled encrypted backup did not complete. Existing backups and source data must be retained. Review https://github.com/Jitpomi/business/actions/runs/${run}. No customer data or credentials are included.`})});
if(!r.ok)throw Error('Backup failure alert not acknowledged');
