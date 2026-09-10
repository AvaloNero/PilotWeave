import http from 'node:http';
import fs from 'node:fs';
import path from 'node:path';

export async function fixtures(repo) {
  const read = name => fs.readFileSync(path.join(repo, 'apps/desktop/src-tauri/tests/fixtures/usage', name));
  const scenario = { prices: 200, billing: 200, runtime: 'available', delay: 0, multiplier: 1 };
  const methods = [], modelRequests = [];
  const server = http.createServer(async (req, res) => {
    const url = new URL(req.url, 'http://127.0.0.1');
    methods.push(url.pathname);
    if (methods.length > 2000) { res.writeHead(429); res.end(); return; }
    if (scenario.delay) await new Promise(resolve => setTimeout(resolve, scenario.delay));
    if (res.destroyed) return;
    let body = {}, status = 200;
    if (url.pathname.startsWith('/model-discovery/')) {
      modelRequests.push({path:url.pathname, method:req.method, bearer:req.headers.authorization === 'Bearer fixture-discovery-key',
        anthropic:req.headers['x-api-key'] === 'fixture-discovery-key', version:req.headers['anthropic-version'] === '2023-06-01'});
      if (url.pathname.includes('/delayed/')) await new Promise(resolve=>setTimeout(resolve,1000));
      if (url.pathname.includes('/unauthorized/')) {status=401;body={error:'PRIVATE_DISCOVERY_ERROR'};}
      else if (url.pathname.includes('/schema/')) body = {changed:true,error:'PRIVATE_DISCOVERY_ERROR'};
      else if (url.pathname.includes('/redirect/')) {res.writeHead(302,{Location:`http://127.0.0.1:${server.address().port}/stolen-key`});res.end();return;}
      else if (url.pathname.includes('/anthropic/')) body=JSON.parse(fs.readFileSync(path.join(repo,'apps/desktop/src-tauri/tests/fixtures/models',url.searchParams.has('after_id')?'anthropic-last-v1.json':'anthropic-list-v1.json')));
      else if (url.pathname.endsWith('/models')) body=JSON.parse(fs.readFileSync(path.join(repo,'apps/desktop/src-tauri/tests/fixtures/models/openai-list-v1.json')));
      else status=404;
    } else if (url.pathname === '/prices') {
      status = scenario.prices;
      body = JSON.parse(read(scenario.latestAliases ? 'openrouter-latest-aliases-v2.json' : 'openrouter-text-prices-v1.json'));
      if (scenario.multiplier !== 1) for (const item of body.data) for (const key of Object.keys(item.pricing)) {
        if (typeof item.pricing[key] === 'string') {
          const value = item.pricing[key];
          if (/^-?\d+(\.\d+)?$/.test(value)) {
            const digits = value.split('.')[1]?.length ?? 0;
            const n = BigInt(value.replace('.','')) * BigInt(scenario.multiplier);
            const sign = n < 0n ? '-' : '';
            const integer = (n < 0n ? -n : n).toString().padStart(digits+1,'0');
            item.pricing[key] = sign + (digits ? integer.slice(0,-digits) + '.' + integer.slice(-digits) : integer);
          }
        }
      }
    } else if (url.pathname === '/github/user') body = { login: 'fixture-user', id: 42 };
    else if (url.pathname === '/github/users/fixture-user') body = { login:'fixture-user',id:42 };
    else if (url.pathname.startsWith('/github/users/fixture-user/settings/billing/')) {
      if(scenario.billing === 'reset') { req.destroy(); return; }
      status = scenario.billing;
      body = { user: 'fixture-user', usageItems: [], timePeriod: { year: Number(url.searchParams.get('year')), month: Number(url.searchParams.get('month')) } };
      if (status === 'schema') { status = 200; body = { unsupported: true }; }
    } else if (url.pathname === '/rpc/connect') body = { protocolVersion: scenario.runtime === 'unsupported' ? 999 : 3, version: 'fixture-3' };
    else if (url.pathname === '/rpc/auth.getStatus') body = { isAuthenticated: scenario.runtime !== 'unauthorized', host: 'github.com', login: 'fixture-user' };
    else if (url.pathname === '/rpc/account.getQuota') body = scenario.runtime === 'schema' ? { changed: true } : scenario.runtime === 'empty' ? { quotaSnapshots: {} } : JSON.parse(read('copilot-quota-v3.json'));
    else if (url.pathname === '/rpc/models.list') body = { models: [{ id: 'gpt-5', name: 'GPT-5' }] };
    else { status = 404; body = {}; }
    res.writeHead(status, { 'Content-Type': 'application/json', ...(status === 429 ? { 'Retry-After': '1' } : {}) });
    res.end(JSON.stringify(body));
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  return { port: server.address().port, scenario, methods, modelRequests, read,
    close: async () => { server.closeAllConnections(); await new Promise(resolve => server.close(resolve)); } };
}
