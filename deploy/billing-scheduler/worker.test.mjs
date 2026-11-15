import test from 'node:test';
import assert from 'node:assert/strict';
import {run} from './worker.js';
const env={BUSINESS_URL:'https://example.com',BILLING_TENANT_ID:'11111111-1111-4111-8111-111111111111',BILLING_TICK_SECRET:'s'.repeat(32)};
test('scheduler rejects redirect without forwarding its credential',async()=>{
 const old=globalThis.fetch;
 globalThis.fetch=async(url,options)=>{assert.equal(url.pathname,`/tenants/${env.BILLING_TENANT_ID}/internal/billing/tick`);assert.equal(options.redirect,'manual');assert.ok(options.signal);return new Response(null,{status:302,headers:{location:'https://other.example'}});};
 try {await assert.rejects(run(env),/HTTP 302/);}finally{globalThis.fetch=old;}
});
test('scheduler rejects reported operation failures',async()=>{
 const old=globalThis.fetch;globalThis.fetch=async()=>Response.json({errors:1});
 try{await assert.rejects(run(env),/1 operations/);}finally{globalThis.fetch=old;}
});

test('scheduler requires an explicit tenant',async()=>{await assert.rejects(run({...env,BILLING_TENANT_ID:undefined}),/tenant UUID/);});
