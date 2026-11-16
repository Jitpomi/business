const test = require('node:test');
const assert = require('node:assert/strict');
const action = require('./post-login.cjs');
const audience='https://business.jitpomi.com/development/api';
async function run(event) { const claims={}; await action.onExecutePostLogin(event,{accessToken:{setCustomClaim:(k,v)=>claims[k]=v}}); return claims; }
test('only signed provider authentication facts become claims',async()=>{
 const claims=await run({resource_server:{identifier:audience},user:{email:'person@example.invalid',email_verified:false,user_metadata:{amr:['mfa'],role:'owner'}},authentication:{methods:[{name:'pwd',timestamp:'2026-11-15T10:00:00Z'},{name:'mfa',timestamp:'2026-11-15T11:00:00Z'}]}});
 assert.equal(claims['https://jitpomi.com/email_verified'],false);
 assert.deepEqual(claims['https://jitpomi.com/amr'],['pwd','mfa']);
 assert.equal(claims['https://jitpomi.com/auth_time'],Date.parse('2026-11-15T10:00:00Z')/1000);
 assert.equal(Object.keys(claims).some(k=>/role|tenant|permission/.test(k)),false);
});
test('missing assurance stays missing and other APIs are unaffected',async()=>{
 assert.deepEqual(await run({resource_server:{identifier:'another-api'}}),{});
 const c=await run({resource_server:{identifier:audience},user:{}});
 assert.equal(c['https://jitpomi.com/amr'],undefined); assert.equal(c['https://jitpomi.com/auth_time'],undefined);
});
