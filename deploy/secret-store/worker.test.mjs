import test from 'node:test';
import assert from 'node:assert/strict';
import worker from './worker.js';
const id = '12345678-1234-4234-8234-123456789abc';
const token = 'x'.repeat(43);
const data = new Map();
const env = { ACCESS_TOKEN: token, SECRETS: { withSession(mode) {
  assert.equal(mode, 'first-primary');
  return { prepare(sql) { return { bind(id, value) { return {
    async first() { return data.has(id) ? { ciphertext:data.get(id) } : null; },
    async run() { assert.match(sql,/ON CONFLICT\(id\) DO NOTHING/); const exists=data.has(id); if(!exists) data.set(id,value); return {meta:{changes:exists?0:1}}; }
  }; } }; } };
} } };
const call=(method='GET', body, auth=`Bearer ${token}`, path=`/secrets/${id}`)=>worker.fetch(new Request(`https://store.example${path}`, {method,headers:{authorization:auth},body}),env);
test('denies unauthenticated reads and writes before accessing persistence',async()=>{
 assert.equal((await call('GET',undefined,'')).status,401);
 assert.equal((await call('PUT','x'.repeat(40),'Bearer wrong')).status,401);
 assert.equal(data.size,0);
});
test('immutable ciphertext survives independent requests without listing or deletion',async()=>{
 const ciphertext='A'.repeat(40);
 assert.equal((await call('PUT',ciphertext)).status,201);
 assert.equal((await call('PUT','B'.repeat(40))).status,409);
 const read=await call(); assert.equal(read.status,200); assert.equal(await read.text(),ciphertext); assert.equal(read.headers.get('cache-control'),'no-store');
 assert.equal((await call('DELETE')).status,405);
 assert.equal((await call('GET',undefined,`Bearer ${token}`,'/secrets')).status,404);
 assert.equal((await call('PUT','plaintext!')).status,400);
 assert.equal((await call('PUT','A'.repeat(262145))).status,413);
});
test('backup snapshot has its own read-only credential and preserves every ciphertext column',async()=>{
 const {DatabaseSync}=await import('node:sqlite');const db=new DatabaseSync(':memory:');db.exec('CREATE TABLE secrets(id TEXT PRIMARY KEY,ciphertext TEXT,created_at TEXT)');db.prepare('INSERT INTO secrets VALUES(?,?,?)').run(id,'A'.repeat(40),'2026-11-18 07:00:00');
 const scoped={
  ACCESS_TOKEN:token,BACKUP_READ_TOKEN:'b'.repeat(43),
  SECRETS:{withSession(mode){
   assert.equal(mode,'first-primary');
   return {prepare(sql){return {async all(){return {results:db.prepare(sql).all()};}};}};
  }}
 };
 const req=(path,auth,method='GET')=>new Request('https://store.example'+path,{method,headers:{authorization:'Bearer '+auth}});
 assert.equal((await worker.fetch(req('/snapshot',token),scoped)).status,401);
 assert.equal((await worker.fetch(req('/secrets/'+id,scoped.BACKUP_READ_TOKEN),scoped)).status,401);
 assert.equal((await worker.fetch(req('/snapshot',scoped.BACKUP_READ_TOKEN,'POST'),scoped)).status,405);
 const response=await worker.fetch(req('/snapshot',scoped.BACKUP_READ_TOKEN),scoped);assert.equal(response.status,200);assert.deepEqual((await response.json()).rows,[{id,ciphertext:'A'.repeat(40),created_at:'2026-11-18 07:00:00'}]);assert.equal(db.prepare('SELECT count(*) AS n FROM secrets').get().n,1);
});
