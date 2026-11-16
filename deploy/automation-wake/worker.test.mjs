import test from 'node:test';
import assert from 'node:assert/strict';
import {run} from './worker.js';
const env={BUSINESS_URL:'https://example.com'};
test('wake requests health without credentials or invoking business work',async()=>{
 const old=globalThis.fetch;
 globalThis.fetch=async(url,options)=>{assert.equal(url.href,'https://example.com/health');assert.equal(options.method,'GET');assert.equal(options.redirect,'manual');assert.equal(options.headers,undefined);return new Response('ok');};
 try{await run(env);}finally{globalThis.fetch=old;}
});
test('wake rejects redirects and failed hosts',async()=>{
 const old=globalThis.fetch;globalThis.fetch=async()=>new Response(null,{status:302});
 try{await assert.rejects(run(env),/HTTP 302/);}finally{globalThis.fetch=old;}
});
test('wake requires a trusted HTTPS origin',async()=>{for(const BUSINESS_URL of ['http://example.com','https://user:password@example.com','https://example.com/path','https://example.com/?token=x']) await assert.rejects(run({BUSINESS_URL}),/HTTPS origin/);});
