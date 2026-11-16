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
