// Wake a sleeping host. Durable database schedules decide what work runs.
export default {
  async scheduled(_event, env, ctx) { ctx.waitUntil(run(env)); },
};
export async function run(env) {
  const base = new URL(env.BUSINESS_URL);
  if (base.protocol !== 'https:' || base.username || base.password || base.search || base.hash || base.pathname !== '/') throw new Error('BUSINESS_URL must be a trusted HTTPS origin');
  const response = await fetch(new URL('/health', base), {
    method: 'GET', redirect: 'manual', signal: AbortSignal.timeout(110000),
  });
  if (!response.ok) throw new Error(`Automation host returned HTTP ${response.status}`);
}
