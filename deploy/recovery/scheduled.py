#!/usr/bin/env python3
"""Daily application checkpoint: read-only source, isolated graph rehearsal, public-key encryption.
The recovery identity and decrypted runtime/vault master key are never needed on this runner.
"""
import argparse, base64, hashlib, importlib.util, json, os, re, shlex, sqlite3, subprocess, sys, time, uuid
from pathlib import Path
ROOT=Path(__file__).resolve().parents[2]
spec=importlib.util.spec_from_file_location('bundle',Path(__file__).with_name('bundle.py'));bundle=importlib.util.module_from_spec(spec);spec.loader.exec_module(bundle)
MAX=32*1024*1024

def run(args, **kwargs):
    p=subprocess.run([str(a) for a in args],capture_output=True,**kwargs)
    if p.returncode:
        message=p.stderr.decode(errors='replace')[-8192:] if isinstance(p.stderr,bytes) else str(p.stderr)[-8192:]
        sensitive=[v for k,v in os.environ.items() if ('TOKEN' in k or 'SECRET' in k or k=='BUSINESS_BACKUP_CONFIG_AGE') and len(v)>4]
        for line in os.environ.get('BUSINESS_BACKUP_DATABASE_ENV','').splitlines():
            if '=' in line:
                try:sensitive+=shlex.split(line.split('=',1)[1])
                except ValueError:pass
        for value in sorted(sensitive,key=len,reverse=True):
            if len(value)>4:message=message.replace(value,'[redacted]')
        message=re.sub(r'Bearer [A-Za-z0-9_.-]+','Bearer [redacted]',message)
        print('Backup command '+Path(str(args[0])).name+' failed (exit '+str(p.returncode)+'): '+message,file=sys.stderr)
        raise RuntimeError('Backup command failed: '+Path(str(args[0])).name)
    return p.stdout

def request(url, token, data=None):
    # Explicitly fixed HTTPS targets, no redirects and no credentials in argv/process logs.
    if not re.fullmatch(r'https://jitpomi-business-(dev-secrets|automation-wake)\.steep-hill-e6f3\.workers\.dev/(snapshot|backup)',url):
        raise ValueError('Unapproved backup target')
    if not re.fullmatch(r'[A-Za-z0-9_-]{32,256}',token):raise ValueError('Invalid backup credential')
    config=f'url = "{url}"\nheader = "Authorization: Bearer {token}"\n'
    if data is not None:config+='request = "POST"\nheader = "Content-Type: application/json"\n'
    args=['curl','--fail','--silent','--show-error','--max-time','60','--max-filesize',str(MAX),'--config','-']
    if data is not None:
        # JSON body is non-secret backup metadata; put it in a private file, never a shell expression.
        args+=['--data-binary','@'+str(data)]
    raw=run(args,input=config.encode())
    if len(raw)>MAX:raise ValueError('Snapshot exceeds bound')
    return json.loads(raw)

def private(path,value):
    fd=os.open(path,os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600)
    with os.fdopen(fd,'wb') as f:f.write(value)

def vault_sql(snapshot):
    if snapshot.get('version')!=1 or not isinstance(snapshot.get('rows'),list):raise ValueError('Invalid vault snapshot')
    db=sqlite3.connect(':memory:');db.executescript((ROOT/'deploy/secret-store/schema.sql').read_text())
    for row in snapshot['rows']:
        uuid.UUID(row['id'])
        if not isinstance(row['created_at'],str):raise ValueError('Missing original creation time')
        db.execute('INSERT INTO secrets(id,ciphertext,created_at) VALUES(?,?,?)',(row['id'],row['ciphertext'],row['created_at']))
    return ('\n'.join(db.iterdump())+'\n').encode()

def monitor_sql(snapshot):
    if snapshot.get('version')!=1 or set(snapshot.get('tables',{}))!={'lease','monitors','outbox'}:raise ValueError('Invalid monitor snapshot')
    db=sqlite3.connect(':memory:');db.executescript((ROOT/'deploy/automation-wake/schema.sql').read_text())
    for table,rows in snapshot['tables'].items():
        columns=[r[1] for r in db.execute(f'PRAGMA table_info({table})')]
        for row in rows:
            if set(row)!=set(columns):raise ValueError('Monitor schema mismatch')
            db.execute(f'INSERT INTO {table}({",".join(columns)}) VALUES({",".join("?" for _ in columns)})',[row[c] for c in columns])
    return ('\n'.join(db.iterdump())+'\n').encode()

def valid_database_configuration(parsed):
    allowed={'TYPEDB_ADDR','TYPEDB_DB','TYPEDB_USERNAME','TYPEDB_PASSWORD','TYPEDB_TLS'}
    versioned=bool(re.fullmatch(r'business_native_dev_v(?:[2-9]|[1-9][0-9]{1,3})',parsed.get('TYPEDB_DB','')))
    return set(parsed)==allowed and versioned and parsed['TYPEDB_TLS']=='true' and parsed['TYPEDB_USERNAME']!='admin'

def checkpoint(destination):
    destination=Path(destination).resolve();destination.mkdir(mode=0o700)
    if run(['git','status','--porcelain'],cwd=ROOT).strip():raise ValueError('Source checkout must be clean; preserve and commit useful work first')
    db_env=os.environ['BUSINESS_BACKUP_DATABASE_ENV'];parsed={}
    for line in db_env.splitlines():
        if line.strip():
            k,v=line.split('=',1);parsed[k]=shlex.split(v)[0]
    if not valid_database_configuration(parsed):raise ValueError('Explicit non-admin TLS development backup credentials required')
    # Refuse a fallback to the application cluster. The local container has only
    # its disposable default account, never cloud credentials or runtime keys.
    local=os.environ['BUSINESS_RECOVERY_LOCAL_ADDR']
    if not re.fullmatch(r'http://127\.0\.0\.1:[1-9][0-9]{0,4}',local) or int(local.rsplit(':',1)[1])>65535:
        raise ValueError('Explicit loopback recovery container required')
    private(destination/'database.env',db_env.encode())
    env={k:v for k,v in os.environ.items() if not k.startswith(('TYPEDB_','BUSINESS_ENV_FILE'))};env['BUSINESS_ENV_FILE']=str(destination/'database.env')
    files=destination/'payload';files.mkdir(mode=0o700);cp=files/'checkpoint';cp.mkdir(mode=0o700)
    binary=Path(os.environ.get('BUSINESS_BACKUP_BIN',str(ROOT/'server/target/debug')))
    started=int(time.time())
    # No retries of a write/commit/provider request. Concurrent source changes fail the checkpoint.
    run([binary/'database_backup','export-online',parsed['TYPEDB_DB'],cp],env=env)
    private(files/'secrets.sql',vault_sql(request('https://jitpomi-business-dev-secrets.steep-hill-e6f3.workers.dev/snapshot',os.environ['BUSINESS_BACKUP_VAULT_TOKEN'])))
    private(files/'monitor.sql',monitor_sql(request('https://jitpomi-business-automation-wake.steep-hill-e6f3.workers.dev/snapshot',os.environ['BUSINESS_BACKUP_MONITOR_TOKEN'])))
    sealed=base64.b64decode(os.environ['BUSINESS_BACKUP_CONFIG_AGE'],validate=True)
    if not sealed.startswith(b'age-encryption.org/v1\n'):raise ValueError('Runtime configuration must already be recipient-encrypted')
    private(files/'configuration.age',sealed)
    # Exact source at this checkpoint, including locked dependencies and recovery tools.
    run(['git','archive','HEAD','-o',files/'business-source.tar'],cwd=ROOT)
    private(files/'RUNBOOK.md',(ROOT/'docs/operations-and-recovery.md').read_bytes())
    recovery_env={k:v for k,v in os.environ.items() if k in {'PATH','HOME','TMPDIR','SSL_CERT_FILE','SSL_CERT_DIR'}}
    recovery_env['BUSINESS_RECOVERY_LOCAL_ADDR']=local
    verification=json.loads(run([binary/'recovery','rehearse-local',cp,parsed['TYPEDB_DB']],env=recovery_env))
    if verification.get('graph_verified') is not True:raise ValueError('Graph rehearsal did not pass')
    vault=sqlite3.connect(':memory:');vault.executescript((files/'secrets.sql').read_text());known={r[0] for r in vault.execute('SELECT id FROM secrets')}
    records=json.loads((cp/(parsed['TYPEDB_DB']+'.records.json')).read_text());references=0
    for record in records.values():
        if record.get('auto_kind')=='connection':
            ref=json.loads(record['auto_payload']).get('secret_ref','')
            if ref:
                if ref.removeprefix('secret:') not in known:raise ValueError('Checkpoint references missing ciphertext')
                references+=1
    metadata={'version':1,'checkpoint_at':started,'source_database':parsed['TYPEDB_DB'],'source_commit':run(['git','rev-parse','HEAD'],cwd=ROOT).decode().strip(),'records':len(records),'ciphertext_rows':len(known),'connection_references':references,'graph_verified':True,'native_schema_verified':True,'rehearsal_location':'isolated runner loopback container','credential_decryption':'Requires separately held recovery identity; verified in operator restore drills'}
    private(files/'checkpoint.json',json.dumps(metadata,indent=2).encode())
    bundle.pack(files,destination/'bundle.zip')
    recipient=os.environ['BUSINESS_BACKUP_RECIPIENT']
    if not re.fullmatch(r'age1[0-9a-z]{58}',recipient):raise ValueError('Invalid public recipient')
    encrypted=destination/'business-backup.age';run([os.environ.get('AGE_BIN','age'),'--encrypt','--recipient',recipient,'--output',encrypted,destination/'bundle.zip'])
    metadata['sha256']=hashlib.sha256(encrypted.read_bytes()).hexdigest();metadata['encrypted_bytes']=encrypted.stat().st_size
    private(destination/'receipt.json',json.dumps(metadata,indent=2).encode())
    print(json.dumps({'encrypted_bytes':metadata['encrypted_bytes'],'checkpoint_at':started,'graph_verified':True}))

if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('directory');a=p.parse_args()
    try:checkpoint(a.directory)
    except Exception as e:
        print('Encrypted backup failed: '+type(e).__name__+'. No success receipt was published; source data and failed rehearsal evidence were retained.',file=sys.stderr);sys.exit(1)
