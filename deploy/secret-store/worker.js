import { timingSafeEqual } from 'node:crypto';
const headers = { 'cache-control': 'no-store', 'content-type': 'text/plain', 'x-content-type-options': 'nosniff' };
const reply = (status, text = '') => new Response(text, { status, headers });
export default {
  async fetch(request, env) {
    // No listing, deletion, browser CORS, or plaintext/decryption capability.
    const url = new URL(request.url);
    const snapshot = url.pathname === '/snapshot';
    const secret = snapshot ? env.BACKUP_READ_TOKEN : env.ACCESS_TOKEN;
    const supplied = request.headers.get('authorization') || '';
    const actualBytes = Buffer.from(supplied);
    const expectedBytes = Buffer.from(`Bearer ${secret}`);
    if (!secret || actualBytes.length !== expectedBytes.length || !timingSafeEqual(actualBytes, expectedBytes)) return reply(401);
    if (snapshot) {
      if (request.method !== 'GET') return reply(405);
      if (url.search) return reply(400);
      const rows = (await env.SECRETS.withSession('first-primary').prepare('SELECT id,ciphertext,created_at FROM secrets ORDER BY id LIMIT 100001').all()).results;
      const body = JSON.stringify({version:1,rows});
      if (rows.length>100000 || body.length>32*1024*1024) return reply(413,'Backup exceeds supported bound; no truncated checkpoint produced');
      return new Response(body,{headers:{...headers,'content-type':'application/json'}});
    }
    const id = /^\/secrets\/([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})$/.exec(url.pathname)?.[1];
    if (!id || url.search) return reply(404);
    try {
      // Primary reads are required: a just-written credential must be readable immediately.
      const db = env.SECRETS.withSession('first-primary');
      if (request.method === 'GET') {
        const row = await db.prepare('SELECT ciphertext FROM secrets WHERE id = ?').bind(id).first();
        return row ? reply(200, row.ciphertext) : reply(404);
      }
      if (request.method !== 'PUT') return reply(405);
      const reader = request.body?.getReader();
      if (!reader) return reply(400);
      const chunks = []; let size = 0;
      while (true) {
        const { done, value } = await reader.read(); if (done) break;
        size += value.length;
        if (size > 262144) { await reader.cancel(); return reply(413); }
        chunks.push(value);
      }
      const bytes = new Uint8Array(size); let offset = 0;
      for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.length; }
      const value = new TextDecoder('utf-8', { fatal: true }).decode(bytes);
      if (value.length < 40 || !/^[A-Za-z0-9+/]+={0,2}$/.test(value)) return reply(400);
      const result = await db.prepare('INSERT INTO secrets (id, ciphertext) VALUES (?, ?) ON CONFLICT(id) DO NOTHING').bind(id, value).run();
      return reply(result.meta.changes === 1 ? 201 : 409);
    } catch { return reply(503); }
  }
};
