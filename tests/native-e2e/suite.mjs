import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { randomUUID, createHash } from 'node:crypto';
import { spawn } from 'node:child_process';
import { DatabaseSync } from 'node:sqlite';
import { NativeDriver, until } from './driver.mjs';
import { fixtures } from './fixture-server.mjs';

const decimal = value => value?.includes('.') ? value.replace(/0+$/, '').replace(/\.$/, '') : value;
const hash = bytes => createHash('sha256').update(bytes).digest('hex');
const query = { start: '2026-09-01', end: '2026-09-09', pageSize: 100 };
export async function nativeSuite(options, record) {
  const server = await fixtures(options.repo);
  const root = path.join(options.privateRoot, `sandbox-${randomUUID()}`);
  fs.mkdirSync(root, { recursive: true });
  fs.writeFileSync(path.join(root, 'context.json'), JSON.stringify({ version: 1, runId: path.basename(root).slice(8), httpPort: server.port }));
  const file = name => path.join(root, name);
  const write = (name, bytes) => { fs.mkdirSync(path.dirname(file(name)), { recursive: true }); fs.writeFileSync(file(name), bytes); };
  const fault = point => write('fault.json', JSON.stringify(point));
  let d;
  const restart = async () => { if (d) await d.stop(); d = new NativeDriver({ ...options, root }); await d.start(); };
  const step = async (name, fn) => { const start = Date.now(); try { const value = await fn(); record(name, 'PASS', value ?? 'Verified through real native IPC', Date.now() - start); }
    catch (e) { record(name, 'FAIL', e.message, Date.now() - start); throw e; } };
  let connection, targets, settings, original, knownPrice;
  const overview = extra => d.ipc('get_usage_overview', { query: { ...query, ...extra } });
  const preview = () => d.ipc('preview_deployment', { connectionId: connection.id, targetIds: targets });
  const apply = plan => d.ipc('apply_deployment_plan', { planId: plan.id, confirmed: true });
  const put = extra => d.ipc('upsert_connection', { input: { id: connection?.id, name: 'Local validation', baseUrl: 'https://example.invalid/v1',
    providerKind: 'openai', protocol: 'chat-completions', headers: {}, models: [{ id: 'model', modelId: 'gpt-5', name: 'GPT-5', enabled: true, capabilities: {} }], ...extra } });
  try {
    await step('B01 Native launch, missing components and Home routes', async () => {
      await restart();
      const status = await d.ipc('get_installation_status');
      assert.ok(status.every(c => c.status === 'missing'));
      const sources = await d.ipc('get_usage_sources'); assert.ok(sources.every(s => !s.enabled));
      await d.browser.$('#content[data-route="overview"]').waitForExist();
      assert.equal(await d.browser.$('.preview-banner').isExisting(), false);
      assert.ok((await d.browser.$('.setup-steps').getText()).length > 0);
      if (options.evidenceRoot) await d.browser.saveScreenshot(path.join(options.evidenceRoot,'native-home.png'));
      return { capability: 'NativeIsolated', webviewVersion: d.browser.capabilities.browserVersion };
    });
    await step('B02 Reviewed install, rediscovery and one-shot plans', async () => {
      await assert.rejects(d.ipc('apply_install_plan', { planId: randomUUID() }));
      const plan = await d.ipc('preview_install', { componentIds: ['vscode'] });
      assert.equal(plan.operations.length, 1); assert.match(plan.operations[0].source, /Microsoft.VisualStudioCode/);
      const one = await d.ipc('apply_install_plan', { planId: plan.id });
      assert.ok(one.results.every(r => r.status === 'completedAndVerified'));
      await assert.rejects(d.ipc('apply_install_plan', { planId: plan.id }));
      const all = await d.ipc('preview_install', { componentIds: [] });
      assert.equal(all.operations.length, 3);
      const result = await d.ipc('apply_install_plan', { planId: all.id });
      assert.ok(result.observations.every(r => r.status === 'ready'));
      assert.equal((await d.ipc('preview_install', { componentIds: [] })).operations.length, 0);
    });
    await step('B26 VS Code probe errors cannot offer installation', async () => {
      write('private/installed-vscode-probe-error','1');
      const status = await d.ipc('get_installation_status');
      assert.equal(status.find(c => c.id === 'vscode-copilot-extension').status, 'unknown');
      await assert.rejects(d.ipc('preview_install', { componentIds: ['vscode-copilot-extension'] }));
      await assert.rejects(d.ipc('preview_install', { componentIds: [] }));
      fs.unlinkSync(file('private/installed-vscode-probe-error'));
      assert.equal((await d.ipc('get_installation_status')).find(c => c.id === 'vscode-copilot-extension').status, 'ready');
    });
    await step('B03 Connection UI to Rust validation and credential isolation', async () => {
      await d.click('#primary-nav [data-route="connections"]');
      await d.click('[data-action="add-connection"]');
      for (const [selector, value] of [['#connection-name', 'Local validation'], ['#base-url', 'https://example.invalid/v1'], ['#api-key', 'sk-isolated-SENTINEL-123456789'], ['#models', 'gpt-5 | GPT-5']]) await d.browser.$(selector).setValue(value);
      await d.click('#save-connection');
      await until(async () => (await d.ipc('get_dashboard')).connections.length === 1, 'Connection did not persist');
      connection = (await d.ipc('get_dashboard')).connections[0];
      assert.equal(connection.hasSecret, true);
      assert.equal(JSON.stringify(connection).includes('SENTINEL'), false);
      for (const input of [{ baseUrl: 'https://user:pass@example.invalid' }, { headers: { X: 'bad\r\nInjected: true' } },
        { models: [{ id: 'a', modelId: 'same', name: 'One', enabled: true }, { id: 'b', modelId: 'same', name: 'Two', enabled: true }] }]) await assert.rejects(put(input));
      const dashboard = await d.ipc('get_dashboard');
      targets = dashboard.clients.filter(c => c.detected).map(c => c.id);
      settings = dashboard.clients.find(c => c.kind === 'vs-code-copilot').path;
      original = Buffer.from('// Foreign comment must remain\n[{"name":"Foreign","vendor":"other","models":[]}]\n');
      fs.writeFileSync(settings, original);
    });
    await step('B04 Reviewed deployment UI, foreign data, backups and manual app', async () => {
      await d.click('[data-action="deploy-connection"]'); await d.click('#preview-deployment');
      const button = await d.browser.$('button=Apply supported changes'); await button.waitForClickable(); await button.click();
      await until(() => fs.readFileSync(settings, 'utf8').includes('pilotWeave'), 'Native deployment did not write');
      await until(async () => { const state = await d.ipc('get_dashboard'); return !state.deploymentRecovery && state.deployments.some(r => r.status === 'applied'); }, 'Deployment audit did not finish');
      assert.match(fs.readFileSync(settings, 'utf8'), /Foreign/);
      assert.deepEqual(fs.readFileSync(settings.replace(/\.json$/, '.json.pilotweave.bak')), original);
      const app = (await d.ipc('get_dashboard')).clients.find(c => c.kind === 'github-copilot-app');
      assert.equal(app.supportsWrite, false);
      assert.equal((await d.ipc('get_setup_status')).manualConfirmed, false);
    });
    await step('B05 No-op bytes, stale targets, unknown, replay and unconfirmed IDs', async () => {
      const before = hash(fs.readFileSync(settings)); const stat = fs.statSync(settings).mtimeMs;
      const p = await preview(); await apply(p);
      assert.equal(hash(fs.readFileSync(settings)), before); assert.equal(fs.statSync(settings).mtimeMs, stat);
      await assert.rejects(apply(p)); await assert.rejects(apply({ id: randomUUID() }));
      const unconfirmed = await preview(); await assert.rejects(d.ipc('apply_deployment_plan', { planId: unconfirmed.id, confirmed: false })); await assert.rejects(apply(unconfirmed));
      const stale = await preview(); fs.appendFileSync(settings, '\n// External edit\n');
      await assert.rejects(apply(stale)); assert.match(fs.readFileSync(settings, 'utf8'), /External edit/);
      const edited = await preview(); connection = await put({ name: 'Changed revision' }); await assert.rejects(apply(edited));
      const key = await preview(); connection = await put({ apiKey: 'sk-isolated-ROTATED-123456789' }); await assert.rejects(apply(key));
    });
    await step('B06 Restart persists native state and synthetic credential reference', async () => {
      const id = connection.id; await restart();
      const dashboard = await d.ipc('get_dashboard'); assert.equal(dashboard.connections[0].id, id);
      assert.equal(dashboard.connections[0].hasSecret, true); await apply(await preview());
    });
    await step('B07 Partial write compensates; audit failure leaves reviewed recovery', async () => {
      connection = await put({ name: 'Fault target', baseUrl: 'https://changed.invalid/v1' });
      const before = hash(fs.readFileSync(settings));
      const p = await preview(); fault('fail:second-write'); const outcome = await apply(p);
      assert.ok(outcome.records.some(r => r.status === 'failed')); assert.equal(hash(fs.readFileSync(settings)), before);
      const audit = await preview(); fault('fail:audit'); await assert.rejects(apply(audit));
      assert.ok((await d.ipc('get_dashboard')).deploymentRecovery);
      const recovery = await d.ipc('preview_deployment_recovery', {});
      await d.ipc('apply_deployment_recovery', { planId: recovery.id, confirmed: true });
      assert.equal(hash(fs.readFileSync(settings)), before);
    });
    await step('B08 Process interruption, conflict refusal and fresh Keep current review', async () => {
      connection = await put({models:[{id:'model',modelId:'gpt-5-crash',name:'Crash model',enabled:true}]});
      const p = await preview(); fault('crash:first-write'); await assert.rejects(apply(p)); await restart();
      assert.ok((await d.ipc('get_dashboard')).deploymentRecovery);
      const restore = await d.ipc('preview_deployment_recovery', {});
      // The first sorted resource is a fake HKCU value. Alter it externally.
      write('private/registry/COPILOT_MODEL', Buffer.from([1, 0, 0, 0, 88, 0, 0, 0]));
      await assert.rejects(d.ipc('apply_deployment_recovery', { planId: restore.id, confirmed: true }));
      const conflict = await d.ipc('preview_deployment_recovery', {}); assert.ok(conflict.view.conflictCount > 0);
      const keep = await d.ipc('preview_deployment_recovery', { action: 'keepCurrent' });
      const before = hash(fs.readFileSync(file('private/registry/COPILOT_MODEL')));
      await d.ipc('apply_deployment_recovery', { planId: keep.id, confirmed: true });
      assert.equal(hash(fs.readFileSync(file('private/registry/COPILOT_MODEL'))), before);
      assert.ok(!(await d.ipc('get_dashboard')).deploymentRecovery);
    });
    await step('B09 Real parsers, exact estimate and repeated import', async () => {
      write('config/PilotWeave/usage-inbox/vscode-otel.jsonl', server.read('vscode-explicit-zero-cache-write.json'));
      write('home/.copilot/session-state/fixture/events.jsonl', server.read('copilot-session-v1.jsonl'));
      await d.ipc('refresh_price_catalog');
      for (const sourceId of ['vscode-otel', 'copilot-session-events']) await d.ipc('set_usage_source_enabled', { sourceId, enabled: true });
      await d.ipc('sync_local_usage', { sourceIds: [] });
      const vs = await overview({ sourceId: 'vscode-otel' });
      assert.equal(decimal(vs.totals.estimateUsd), '0.00075'); assert.equal(vs.totals.cacheWrite, '0'); assert.equal(decimal(vs.totals.cacheHitRate), '0.8');
      knownPrice = vs.totals.priceSnapshots[0];
      const before = await overview(); await d.ipc('sync_local_usage', { sourceIds: [] });
      assert.deepEqual((await overview()).totals, before.totals);
      assert.ok((await overview({ sourceId: 'copilot-session-events' })).records.some(r => r.normalizedInput === null));
      await d.click('#primary-nav [data-route="usage"]');await d.click('#refresh-button');
      await d.click('[data-usage-tab="sources"]');
      await d.click('[data-usage-action="sync"]');
      await until(async()=>!(await d.browser.$('[data-usage-action="sync"]').getText()).includes('Importing'),'UI import did not finish');
      await d.browser.$('#usage-filters select[name="sourceId"]').selectByAttribute('value','vscode-otel');
      await d.click('#usage-filters button[type="submit"]');
      await until(async()=>(await d.browser.$('.usage-metrics > div:first-child strong').getText())==='1000','Native Usage metrics did not refresh');
      assert.equal(await d.browser.$('.usage-metrics > div:nth-child(5) strong').getText(),'0');
      assert.match(await d.browser.$('.usage-metrics > div:last-child strong').getText(),/^\$0\.000750*$/);
      await d.click('[data-usage-tab="models"]');
      await until(async()=>(await d.browser.$('.model-table').getText()).includes('$0.00075'),'Model table did not render native estimate');
      await d.browser.$('.usage-metrics').scrollIntoView({block:'center'});
      if (options.evidenceRoot) await d.browser.saveScreenshot(path.join(options.evidenceRoot,'native-usage.png'));
    });
    await step('B10 Incomplete tail, rotation, truncation and exact large counters', async () => {
      const row = JSON.parse(server.read('vscode-explicit-zero-cache-write.json'));
      row._spanContext.spanId = 'abcdef0123456789'; row.attributes['gen_ai.usage.input_tokens'] = 1000000000000;
      delete row.attributes['gen_ai.usage.cache_creation.input_tokens'];
      const text = JSON.stringify(row);
      fs.appendFileSync(file('config/PilotWeave/usage-inbox/vscode-otel.jsonl'), text.slice(0, -3));
      await d.ipc('sync_local_usage', { sourceIds: ['vscode-otel'] }); assert.equal((await overview({ sourceId: 'vscode-otel' })).totalRecords, 1);
      fs.appendFileSync(file('config/PilotWeave/usage-inbox/vscode-otel.jsonl'), text.slice(-3) + '\n');
      await d.ipc('sync_local_usage', { sourceIds: ['vscode-otel'] });
      const after = await overview({ sourceId: 'vscode-otel' }); assert.equal(after.totalRecords, 2);
      assert.equal(after.records.find(r => r.inputReported === '1000000000000')?.inputReported, '1000000000000');
      fs.renameSync(file('config/PilotWeave/usage-inbox/vscode-otel.jsonl'), file('config/PilotWeave/usage-inbox/rotated.jsonl'));
      write('config/PilotWeave/usage-inbox/vscode-otel.jsonl', text + '\n');
      await d.ipc('sync_local_usage', { sourceIds: ['vscode-otel'] }); assert.equal((await overview({ sourceId: 'vscode-otel' })).totalRecords, 2);
      write('config/PilotWeave/usage-inbox/vscode-otel.jsonl', ''); await d.ipc('sync_local_usage', { sourceIds: ['vscode-otel'] });
      assert.equal((await overview({ sourceId: 'vscode-otel' })).totalRecords, 2);
    });
    await step('B11 Price A-B-A and immutable historical bindings', async () => {
      await restart(); server.scenario.multiplier = 2; await d.ipc('refresh_price_catalog');
      const b = (await d.ipc('get_price_catalog')).catalog;
      const row = JSON.parse(server.read('vscode-explicit-zero-cache-write.json')); row._spanContext.spanId = '0123012301230123';
      write('config/PilotWeave/usage-inbox/vscode-otel.jsonl', JSON.stringify(row) + '\n'); await d.ipc('sync_local_usage', { sourceIds: ['vscode-otel'] });
      const before = (await overview({ sourceId: 'vscode-otel' })).records;
      assert.ok(before.some(r => r.priceSnapshotId !== knownPrice && decimal(r.estimateUsd) === '0.0015'));
      await restart(); server.scenario.multiplier = 1; const a = await d.ipc('refresh_price_catalog');
      assert.equal(a.catalog.id, knownPrice); assert.notEqual(b.id, knownPrice); assert.deepEqual((await overview({ sourceId: 'vscode-otel' })).records, before);
    });
    await step('B12 Runtime zero, unknown, empty, schema and unauthorized states', async () => {
      let value = await d.ipc('refresh_official_runtime_usage'); assert.equal(value.latest.status, 'available');
      assert.equal(value.latest.quotas.find(q => q.key === 'premium_interactions').used, '0');
      assert.equal(value.latest.quotas.find(q => q.key === 'chat').used, null);
      for (const [mode, status] of [['empty', 'successfulEmpty'], ['schema', 'schemaError'], ['unsupported', 'unsupported'], ['unauthorized', 'unauthorized']]) {
        await restart(); server.scenario.runtime = mode; value = await d.ipc('refresh_official_runtime_usage'); assert.equal(value.latest.status, status);
      }
      assert.ok(value.lastSuccessful); assert.equal(value.stale, true); server.scenario.runtime = 'available';
    });
    await step('B13 Personal Billing authorization, family errors and period isolation', async () => {
      await assert.rejects(d.ipc('refresh_personal_github_usage', { year: 2026, month: 9 }));
      await d.ipc('authorize_github', { token: 'github_pat_ISOLATED_FIXTURE_TOKEN_123456789' });
      for (const [http, status] of [[200, 'successfulEmpty'], [401, 'unauthorized'], [403, 'insufficientPermission'], [404, 'notCovered'], [429, 'rateLimited'], ['schema', 'schemaError'], [500, 'unavailable'], ['reset', 'networkError']]) {
        await restart(); server.scenario.billing = http;
        // A fresh native authorization epoch also avoids carrying a prior cooldown.
        if (http === 'schema') await new Promise(r => setTimeout(r, 1100));
        const view = await d.ipc('refresh_personal_github_usage', { year: 2026, month: 9 });
        assert.ok(view.families.every(f => f.latest?.status === status), JSON.stringify(view.families.map(f => f.latest?.status)));
      }
      const prior = await d.ipc('get_github_billing', { year: 2026, month: 8 }); assert.ok(prior.families.every(f => !f.latest));
      server.scenario.billing = 200;
    });
    await step('B14 Failed writes and concurrent cancel leave terminal job states', async () => {
      await restart(); fault('fail:price-save'); await assert.rejects(d.ipc('refresh_price_catalog'));
      fault('fail:runtime-save'); await assert.rejects(d.ipc('refresh_official_runtime_usage'));
      const runs = await d.ipc('get_usage_runs'); assert.ok(runs.every(r => r.status !== 'inProgress'));
      assert.ok(runs.filter(r => ['official-runtime', 'price-catalog'].includes(r.sourceId)).some(r => r.status === 'unavailable'));
      await restart(); server.scenario.delay = 750;
      // Run commands concurrently inside one WebView execution (WebDriver itself serializes requests).
      const canceled = await d.browser.executeAsync(done => {
        const invoke = window.__TAURI__.core.invoke;
        const first = invoke('refresh_price_catalog');
        setTimeout(() => { invoke('refresh_price_catalog').then(() => done({ duplicate: true }), async () => {
          await invoke('cancel_usage_sync'); await first; done({ duplicate: false, runs: await invoke('get_usage_runs') });
        }); }, 100);
      });
      assert.equal(canceled.duplicate, false); assert.ok(canceled.runs.some(r => r.status === 'canceled'));
      assert.ok(canceled.runs.every(r => r.status !== 'inProgress')); server.scenario.delay = 0;
    });
    await step('B15 Privacy metadata scan and opt-in survives restart', async () => {
      const payload = JSON.stringify([await overview(), await d.ipc('get_dashboard'), await d.ipc('get_usage_runs')]);
      assert.doesNotMatch(payload, /PRIVATE_PROMPT|PRIVATE_TOKEN|PRIVATE_ENV|PRIVATE_TOOL|SENTINEL|ROTATED/);
      await restart(); assert.ok((await d.ipc('get_usage_sources')).find(s => s.id === 'vscode-otel').enabled);
      const directory = file('config/PilotWeave');
      for (const name of fs.readdirSync(directory)) if (/state|usage\.sqlite3/.test(name) && fs.statSync(path.join(directory, name)).isFile()) {
        assert.doesNotMatch(fs.readFileSync(path.join(directory, name)).toString('latin1'), /PRIVATE_PROMPT|PRIVATE_TOKEN|PRIVATE_ENV|PRIVATE_TOOL|SENTINEL|ROTATED/);
      }
      assert.ok(server.methods.every(p => /^\/(rpc|prices|github|rejected)\b/.test(p)));
    });
    await step('B16 Clear and delete require explicit operation; source files preserved', async () => {
      await assert.rejects(d.ipc('clear_local_usage', { sourceId: 'vscode-otel', confirmed: false }));
      const before = fs.readFileSync(file('config/PilotWeave/usage-inbox/vscode-otel.jsonl'));
      await d.ipc('clear_local_usage', { sourceId: 'vscode-otel', confirmed: true });
      assert.equal((await overview({ sourceId: 'vscode-otel' })).totalRecords, 0);
      assert.deepEqual(fs.readFileSync(file('config/PilotWeave/usage-inbox/vscode-otel.jsonl')), before);
      await d.ipc('delete_connection', { connectionId: connection.id }); assert.equal((await d.ipc('get_dashboard')).connections.length, 0);
    });
    await step('B17 Aggregated counters remain exact above JavaScript safe integer', async () => {
      await d.ipc('set_usage_source_enabled',{sourceId:'vscode-otel',enabled:true});
      const row = JSON.parse(server.read('vscode-explicit-zero-cache-write.json'));
      row.attributes['gen_ai.usage.input_tokens'] = 999999999999;
      let rows = '';
      for (let i = 0; i < 9008; i++) { row._spanContext.spanId = (10000 + i).toString(16).padStart(16,'0'); rows += JSON.stringify(row) + '\n'; }
      write('config/PilotWeave/usage-inbox/vscode-otel.jsonl',rows);
      await d.ipc('sync_local_usage',{sourceIds:['vscode-otel']});
      const result = await overview({sourceId:'vscode-otel'});
      assert.equal(result.totalRecords,9008); assert.equal(result.totals.inputReported,(9008n * 999999999999n).toString());
    });
    await step('B18 Import interruption retains committed cursor and recovers once', async () => {
      const row = JSON.parse(server.read('vscode-explicit-zero-cache-write.json'));row._spanContext.spanId='ffffffffffffffff';
      write('config/PilotWeave/usage-inbox/vscode-otel.jsonl',JSON.stringify(row)+'\n');fault('crash:import-commit');
      await assert.rejects(d.ipc('sync_local_usage',{sourceIds:['vscode-otel']}));await restart();
      assert.ok((await d.ipc('get_usage_runs')).some(r=>r.status==='interrupted'));
      await d.ipc('sync_local_usage',{sourceIds:['vscode-otel']});assert.equal((await overview({sourceId:'vscode-otel'})).totalRecords,9009);
    });
    await step('B19 Corrupt and future SQLite are unavailable and never recreated', async () => {
      await d.stop();const dbfile=file('config/PilotWeave/usage.sqlite3');const original=fs.readFileSync(dbfile);
      fs.writeFileSync(dbfile,'corrupt database sentinel');const damaged=hash(fs.readFileSync(dbfile));await restart();
      assert.equal((await d.ipc('get_dashboard')).usageDb.state,'unavailable');assert.equal(hash(fs.readFileSync(dbfile)),damaged);
      await assert.rejects(overview());await d.stop();fs.writeFileSync(dbfile,original);
      const sqlite=new DatabaseSync(dbfile);sqlite.exec('PRAGMA user_version=999');sqlite.close();const newer=hash(fs.readFileSync(dbfile));await restart();
      assert.equal((await d.ipc('get_dashboard')).usageDb.state,'unavailable');assert.equal(hash(fs.readFileSync(dbfile)),newer);
      await d.stop();fs.writeFileSync(dbfile,original);await restart();assert.equal((await d.ipc('get_dashboard')).usageDb.state,'available');
    });
    await step('B20 Primary and last-good recovery forbid native mutations', async () => {
      await d.stop();const primary=file('config/PilotWeave/state.json'),good=primary+'.last-good';
      const before=fs.readFileSync(primary),backup=fs.readFileSync(good);fs.writeFileSync(primary,'{corrupt');await restart();
      assert.ok((await d.ipc('get_dashboard')).stateRecovery);await assert.rejects(put({id:null}));assert.equal(fs.readFileSync(primary,'utf8'),'{corrupt');
      await d.stop();fs.writeFileSync(good,'{corrupt');await restart();assert.ok((await d.ipc('get_dashboard')).stateRecovery);await assert.rejects(put({id:null}));
      await d.stop();fs.writeFileSync(primary,before);fs.writeFileSync(good,backup);await restart();assert.ok(!(await d.ipc('get_dashboard')).stateRecovery);
    });
    await step('B21 Cross-process managed write lease rejects a competing mutation', async () => {
      const lease=file('config/PilotWeave/managed-writes.lock').replaceAll("'","''");
      const ps="$f=[IO.File]::Open('"+lease+"',[IO.FileMode]::Open,[IO.FileAccess]::ReadWrite,[IO.FileShare]::None); [Console]::WriteLine('READY'); [Console]::Out.Flush(); [Console]::ReadLine() | Out-Null; $f.Dispose()";
      const holder=spawn('powershell.exe',['-NoProfile','-NonInteractive','-Command',ps],{windowsHide:true,stdio:['pipe','pipe','pipe']});
      const exited=new Promise(resolve=>holder.once('exit',resolve));let ready=false;holder.stdout.on('data',b=>{if(b.toString().includes('READY'))ready=true;});
      try {await until(()=>ready,'Lease holder failed');await assert.rejects(put({id:null}));}
      finally {holder.stdin.end('\n');await exited;}
      connection=await put({id:null});assert.ok(connection.id);await d.ipc('delete_connection',{connectionId:connection.id});
    });
    await step('B22 Installer zero exit without rediscovery is not success', async () => {
      fs.unlinkSync(file('private/installed-vscode'));write('private/installed-false-success','1');
      const plan=await d.ipc('preview_install',{componentIds:['vscode']});const result=await d.ipc('apply_install_plan',{planId:plan.id});
      assert.equal(result.results[0].status,'processSucceededVerificationFailed');fs.unlinkSync(file('private/installed-false-success'));
      const retry=await d.ipc('preview_install',{componentIds:['vscode']});await d.ipc('apply_install_plan',{planId:retry.id});
    });
    await step('B23 Account confirmation expiry and changed evidence stay distinct from verification', async () => {
      const status = await d.ipc('get_setup_status');
      const input = {login:'fixture-user',evidenceFingerprint:status.evidenceFingerprint,confirmed:true};
      await assert.rejects(d.ipc('confirm_account_alignment',{...input,confirmed:false}));
      await assert.rejects(d.ipc('confirm_account_alignment',{...input,evidenceFingerprint:'stale'}));
      await d.ipc('confirm_account_alignment',input);
      assert.equal((await d.ipc('get_setup_status')).alignment,'userConfirmedSameAccount');
      await restart();assert.equal((await d.ipc('get_setup_status')).alignment,'userConfirmedSameAccount');
      await d.stop();
      const sqlite = new DatabaseSync(file('config/PilotWeave/usage.sqlite3'));
      try {
        const row=sqlite.prepare('SELECT payload FROM setup_preferences WHERE id=1').get();
        const payload=JSON.parse(row.payload);payload.accountConfirmation.confirmedAt='2000-01-01T00:00:00Z';
        sqlite.prepare('UPDATE setup_preferences SET payload=? WHERE id=1').run(JSON.stringify(payload));
      } finally {sqlite.close();}
      await restart();assert.notEqual((await d.ipc('get_setup_status')).alignment,'userConfirmedSameAccount');
      await d.ipc('confirm_account_alignment',{...input,evidenceFingerprint:(await d.ipc('get_setup_status')).evidenceFingerprint});
      write('private/installed-signed-in','1');
      const changed=await d.ipc('get_setup_status');
      assert.equal(changed.accounts.anchor.state,'verified');assert.notEqual(changed.alignment,'userConfirmedSameAccount');
      await assert.rejects(d.ipc('confirm_account_alignment',input));
    });
    await step('B24 Authorization changes discard late Billing responses', async () => {
      await restart();server.scenario.billing=200;server.scenario.delay=500;
      try {
        const result = await d.browser.executeAsync(done => {
          const invoke=window.__TAURI__.core.invoke;
          const pending=invoke('refresh_personal_github_usage',{year:2026,month:8}).then(()=>({accepted:true}),error=>({accepted:false,error:String(error)}));
          setTimeout(async()=>{try {await invoke('clear_github_authorization');done({...await pending,runs:await invoke('get_usage_runs')});}catch(error){done({failure:String(error)});}},100);
        });
        assert.equal(result.accepted,false);assert.match(result.error,/Authorization changed/);
        assert.ok(result.runs.some(r=>r.sourceId==='personal-billing'&&r.status==='canceled'));
        assert.ok(result.runs.every(r=>r.status!=='inProgress'));
        await d.stop();const sqlite=new DatabaseSync(file('config/PilotWeave/usage.sqlite3'),{readOnly:true});
        try {assert.equal(sqlite.prepare("SELECT count(*) AS n FROM github_billing_snapshots WHERE period_start LIKE '2026-08-%'").get().n,0);}finally {sqlite.close();}
      } finally {server.scenario.delay=0;}
    });
    await step('B25 Latest model aliases use parser v2 with exact identity through IPC', async () => {
      server.scenario.latestAliases=true;await restart();
      const prices=await d.ipc('refresh_price_catalog');assert.equal(prices.catalog.parserVersion,2);
      const row=JSON.parse(server.read('vscode-explicit-zero-cache-write.json'));
      row._spanContext.spanId='abaaaaaaaaaaaaab';row.attributes['gen_ai.request.model']='~openai/gpt-latest';
      row.attributes['gen_ai.response.model']='~openai/gpt-latest';
      write('config/PilotWeave/usage-inbox/vscode-otel.jsonl',JSON.stringify(row)+'\n');
      await d.ipc('sync_local_usage',{sourceIds:['vscode-otel']});
      const selected=await overview({sourceId:'vscode-otel',model:'~openai/gpt-latest'});
      assert.equal(selected.totalRecords,1);assert.equal(selected.records[0].canonicalModel,'~openai/gpt-latest');
      assert.equal(selected.records[0].priceSnapshotId,prices.catalog.id);
    });
    await step('B27 Automatic provider model discovery, selection and saved credentials through real IPC', async () => {
      await d.click('#primary-nav [data-route="connections"]');
      await d.click('[data-action="add-connection"]');
      for (const [selector,value] of [['#connection-name','Discovered connection'],['#base-url','https://models.example.invalid/v1'],['#api-key','fixture-discovery-key'],['#models','manual-model | Keep this name']]) await d.browser.$(selector).setValue(value);
      await until(async()=>await d.browser.$('#model-discovery').getAttribute('data-status')==='available','Automatic model discovery did not finish');
      assert.equal((await d.browser.$$('[data-model-list] label')).length,2);
      assert.ok(server.modelRequests.some(r=>r.method==='GET'&&r.bearer&&r.path==='/model-discovery/v1/models'));
      await d.click('[data-model-list] input[type="checkbox"]');
      await d.click('[data-model-add]');
      assert.equal(await d.browser.$('#models').getValue(),'manual-model | Keep this name\nfixture/chat | fixture/chat');
      if(options.evidenceRoot) await d.browser.saveScreenshot(path.join(options.evidenceRoot,'native-model-discovery.png'));
      await d.click('#save-connection');
      await until(async()=>(await d.ipc('get_dashboard')).connections.some(c=>c.name==='Discovered connection'),'Discovered connection did not save');
      const saved=(await d.ipc('get_dashboard')).connections.find(c=>c.name==='Discovered connection');
      assert.equal(saved.hasSecret,true);assert.equal(saved.models.find(m=>m.modelId==='fixture/chat').capabilities.toolCalling,undefined);
      const input={connectionId:saved.id,baseUrl:saved.baseUrl,providerKind:saved.providerKind,protocol:saved.protocol,headers:{},apiKey:null,clearSecret:false};
      assert.equal((await d.ipc('discover_connection_models',{input})).status,'available');
      const before=server.modelRequests.length;
      assert.equal((await d.ipc('discover_connection_models',{input:{...input,baseUrl:'https://models.example.invalid/other'}})).status,'credentialRequired');
      assert.equal(server.modelRequests.length,before);
      for(const [route,status] of [['unauthorized','unauthorized'],['schema','schemaError'],['redirect','unsupported']]) {
        const result=await d.ipc('discover_connection_models',{input:{...input,apiKey:'fixture-discovery-key',baseUrl:`https://models.example.invalid/${route}/v1`}});
        assert.equal(result.status,status);assert.equal(result.models.length,0);assert.doesNotMatch(JSON.stringify(result),/PRIVATE_DISCOVERY|fixture-discovery-key/);
      }
      const anthropic=await d.ipc('discover_connection_models',{input:{...input,apiKey:'fixture-discovery-key',baseUrl:'https://models.example.invalid/anthropic/v1/messages',protocol:'messages',providerKind:'anthropic'}});
      assert.equal(anthropic.status,'available');assert.equal(anthropic.models.length,2);
      assert.equal(server.modelRequests.filter(r=>r.anthropic&&r.version).length,2);
      assert.ok(!server.methods.includes('/stolen-key'));
      await d.click(`[data-action="edit-connection"][data-id="${saved.id}"]`);
      await d.browser.$('#api-key').setValue('fixture-discovery-key');
      await d.browser.$('#base-url').setValue('https://models.example.invalid/unauthorized/v1');
      await d.click('[data-model-fetch]');
      await until(async()=>await d.browser.$('#model-discovery').getAttribute('data-status')==='unauthorized','Failure status was not shown');
      assert.match(await d.browser.$('#models').getValue(),/manual-model \| Keep this name/);
      await d.browser.$('#base-url').setValue('https://models.example.invalid/delayed/v1');
      await d.click('[data-model-fetch]');
      await d.browser.$('#base-url').setValue('');
      await new Promise(resolve=>setTimeout(resolve,1400));
      assert.equal(await d.browser.$('#model-discovery').getAttribute('data-status'),'idle');
      assert.equal(await d.browser.$('[data-model-results]').isDisplayed(),false);
      await d.click('.modal-header [data-modal-close]');
      await d.ipc('delete_connection',{connectionId:saved.id});
      return {native:'WebView2 -> Tauri IPC -> Rust GET -> local HTTP fixtures',credentials:'synthetic only',requests:server.modelRequests.length};
    });
    await step('B28 Home add/edit/select/deployment preview stays on Home without automatic deployment', async () => {
      await d.click('#primary-nav [data-route="overview"]');
      const before=(await d.ipc('get_dashboard')).deployments.length;
      await d.click('.setup-connection-card [data-action="add-connection"]');
      for(const [selector,value] of [['#connection-name','Home connection'],['#base-url','https://models.example.invalid/v1'],['#models','home-model | Home Model']]) await d.browser.$(selector).setValue(value);
      await d.click('#save-connection');
      await until(async()=>(await d.ipc('get_dashboard')).connections.some(c=>c.name==='Home connection'),'Home connection did not save');
      let current=(await d.ipc('get_dashboard')).connections.find(c=>c.name==='Home connection');
      await until(async()=>(await d.ipc('get_setup_status')).preferences.connectionId===current.id,'Home did not select newly saved connection');
      await until(async()=>await d.browser.$('#setup-connection').getValue()===current.id,'Home selection was not rendered');
      assert.equal(await d.browser.$('#content').getAttribute('data-route'),'overview');
      await d.click('.setup-connection-card [data-action="edit-connection"]');
      assert.equal(await d.browser.$('#base-url').getValue(),current.baseUrl);
      await d.browser.$('#models').setValue('home-model | Renamed Home Model\nsecond-model | Second Model');
      await d.click('#save-connection');
      await until(async()=>(await d.ipc('get_dashboard')).connections.find(c=>c.id===current.id).models.length===2,'Home edit did not save');
      await until(async()=>(await d.browser.$('.setup-connection-card').getText()).includes('2 enabled models'),'Home model summary did not refresh');
      assert.equal((await d.ipc('get_dashboard')).deployments.length,before);
      if(options.evidenceRoot) await d.browser.saveScreenshot(path.join(options.evidenceRoot,'native-home-connections.png'));
      await d.click('.setup-connection-card [data-action="setup-deploy"]');
      await d.click('#preview-deployment');
      await d.browser.$('button=Apply supported changes').waitForExist();
      assert.equal((await d.ipc('get_dashboard')).deployments.length,before);
      await d.click('.modal-header [data-modal-close]');
      await d.ipc('delete_connection',{connectionId:current.id});
    });
    await step('B29 VS Code sign-in never spawns and scopes the reviewed client', async () => {
      await d.click('#primary-nav [data-route="overview"]');
      const checks=await d.browser.$('.setup-checks');
      if(await checks.getAttribute('open')===null) await d.click('.setup-checks > summary');
      await d.click('[data-action="setup-signin"][data-surface="vsCodeCopilot"]');
      await d.browser.$('.account-plan-operation').waitForExist();
      assert.equal((await d.browser.$$('.account-plan-operation')).length,1);
      await d.click('.account-modal .modal-header [data-account-modal-close]');
      for(const open of [false,true,false]) {
        if(open) write('private/vscode-window-open','1');
        else if(fs.existsSync(file('private/vscode-window-open'))) fs.unlinkSync(file('private/vscode-window-open'));
        // The spawn fault must remain unconsumed throughout all VS Code actions.
        fault('fail:login');
        const plan=await d.ipc('preview_login',{surfaces:['vsCodeCopilot']});
        assert.equal(plan.operations.length,1);
        const result=await d.ipc('apply_login_plan',{planId:plan.id});
        assert.equal(result.run.steps[0].status,'actionRequired');
        assert.match(result.run.steps[0].detail,open?/existing VS Code window/:/Open VS Code yourself/);
        assert.equal(fs.existsSync(file('fault.json')),true);
        await assert.rejects(d.ipc('apply_login_plan',{planId:plan.id}));
        fs.unlinkSync(file('fault.json'));
      }
    });
  } finally {
    try { await d?.stop(); await server.close(); record('B cleanup: owned driver, app and loopback ports', 'PASS', 'Closed owned process tree; fixtures retained privately'); }
    catch (e) { record('B cleanup: owned driver, app and loopback ports', 'FAIL', e.message); }
  }
}
