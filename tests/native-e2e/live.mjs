import fs from 'node:fs';
import path from 'node:path';
import assert from 'node:assert/strict';
import { execFileSync, spawn } from 'node:child_process';
import { NativeDriver } from './driver.mjs';
import { HostWindowWatch } from './host-windows.mjs';
import { digest } from '../../scripts/validation-report.mjs';

export function acceptGate(manifest, action, completedAction, detail) {
  const scope=digest(JSON.stringify(detail));
  manifest.approvedScopes ??= {};
  if (manifest.completed.includes(action) && manifest.approvedScopes[action] === scope) return true;
  if (completedAction === action && manifest.pendingAction?.action === action && manifest.pendingAction.scope === scope) {
    if(!manifest.completed.includes(action))manifest.completed.push(action);
    manifest.approvedScopes[action]=scope;manifest.pendingAction = null;return true;
  }
  manifest.pendingAction = { action, detail, scope };return false;
}
const states = value => ({ components: (Array.isArray(value) ? value : value.components)?.map(c => ({ id:c.id,status:c.status,version:c.version })), observedAt:value.observedAt });
export function enrollDedicatedProfile({ profile, appData, runId, readSid = () => execFileSync('whoami.exe',
  ['/user','/fo','csv','/nh'],{encoding:'utf8',windowsHide:true,timeout:5000,maxBuffer:4096}) }, record) {
  const blocked = detail => { record('C2 Dedicated profile preflight','BLOCKED',detail); return false; };
  if (!profile || !appData || !path.isAbsolute(profile) || !path.isAbsolute(appData)) return blocked('Current profile paths are unavailable.');
  let sid;
  try { sid = readSid().match(/^"(?:[^"\r\n]|"")*","(S-1-5-\d+(?:-\d+)*)"\s*$/m)?.[1]; } catch {}
  if (!sid) return blocked('Could not determine the current user SID. Retry after the official Windows identity command is available.');
  const marker=path.join(profile,'.pilotweave-validation-user.json');
  try {
    if (!fs.existsSync(marker)) {
      const existing=[path.join(profile,'.copilot'),path.join(appData,'Code'),path.join(appData,'PilotWeave')].some(p=>fs.existsSync(p));
      if (existing) return blocked('Profile already contains client/application data. This mode cannot enroll the existing host profile.');
      fs.writeFileSync(marker,JSON.stringify({version:1,sid,runId}),{flag:'wx'});
    }
    const stat=fs.lstatSync(marker);
    if (!stat.isFile() || stat.isSymbolicLink() || stat.size > 1024) return blocked('Dedicated profile enrollment marker is invalid; preserve it and use a separate clean profile.');
    const owner=JSON.parse(fs.readFileSync(marker,'utf8'));
    if (owner?.version !== 1 || owner.sid !== sid || owner.runId !== runId) return blocked('Dedicated profile belongs to another user or run. Resume the original RunId in its original profile, or use a separate clean profile for a new run. Keep the enrollment marker.');
    return true;
  } catch { return blocked('Dedicated profile enrollment marker could not be verified. Preserve it and resume the original run after resolving access, or use a separate clean profile.'); }
}
export async function liveAcceptance({ runRoot, manifest, completedAction }, record) {
  const save = () => fs.writeFileSync(path.join(runRoot,'live-manifest.json'),JSON.stringify(manifest,null,2));
  let driver;
  let windows;
  const start = async application => { driver = new NativeDriver({ application,...manifest.driverOptions }); await driver.start(); return driver; };
  const gate = (action, detail) => {
    if (acceptGate(manifest,action,completedAction,detail)) { save(); return true; }
    save(); record(`C action required: ${action}`,'BLOCKED',detail); return false;
  };
  try {
    if (manifest.target === 'HostObserve') {
      windows = new HostWindowWatch();
      await windows.start();
      await windows.normalStartup(manifest.artifact.application);
      record('C1 Normal desktop startup without VS Code windows','PASS',{transport:'Direct EXE, no arguments, no WebDriver',newCodeWindows:windows.added.size});
      await start(manifest.artifact.application);
      await assert.rejects(driver.ipc('local_e2e_status'));
      record('C1 Default EXE startup and no test IPC','PASS',{sha256:manifest.artifact.sha256,webviewVersion:driver.browser.capabilities.browserVersion,driverInitialUrl:driver.initialUrl});
      const installs=await driver.ipc('get_installation_status');
      record('C1 Real installed components',installs.every(c=>c.status==='ready')?'PASS':'BLOCKED',states(installs));
      for (let i=0;i<3;i++) {
        assert.deepEqual(await driver.ipc('get_installation_status'),installs);
        windows.assertClean();
      }
      if (manifest.expectedComponents) {
        for (const expected of manifest.expectedComponents) {
          const actual = installs.find(c=>c.id===expected.id);
          assert.equal(actual?.status,expected.status);
          assert.equal(actual?.version,expected.version);
        }
        record('C1 Components match independently recorded host evidence','PASS',manifest.expectedComponents);
      }
      record('C1 Repeated discovery without VS Code windows','PASS',{repeats:3,newCodeWindows:windows.added.size});
      const accounts=await driver.ipc('get_account_status');
      record('C1 Official account observations','PASS',{anchor:accounts.anchor.state,surfaces:accounts.surfaces.map(s=>({surface:s.surface,state:s.state,evidence:s.evidence}))});
      const before=await driver.ipc('get_dashboard');
      const sources=await driver.ipc('get_usage_sources');
      const prices=await driver.ipc('refresh_price_catalog');
      record('C1 Public price request',prices.latest?.status==='available'?'PASS':'BLOCKED',{capability:prices.latest?.status ?? (prices.catalog ? 'available':'unavailable'),source:prices.catalog?.source,detail:prices.latest?.detail});
      const runtime=await driver.ipc('refresh_official_runtime_usage');
      record('C1 Official read-only runtime RPC',runtime.latest?.status==='available'&&runtime.latest?.modelStatus==='available'?'PASS':'BLOCKED',{capability:runtime.latest?.status,modelStatus:runtime.latest?.modelStatus,detail:runtime.latest?.detail});
      const authorization=await driver.ipc('get_github_authorization_status');
      record('C1 Separate personal Billing capability',authorization.state==='verified'?'PASS':'BLOCKED',{capability:authorization.state,detail:'Separate PilotWeave authorization only; no client token copied'});
      const after=await driver.ipc('get_dashboard');
      assert.deepEqual(after.connections,before.connections); assert.deepEqual(after.deployments,before.deployments);
      assert.deepEqual((await driver.ipc('get_usage_sources')).map(s=>[s.id,s.enabled]),sources.map(s=>[s.id,s.enabled]));
      await driver.stop();await start(manifest.artifact.application);
      assert.deepEqual((await driver.ipc('get_dashboard')).connections,before.connections);
      assert.equal((await driver.ipc('get_official_runtime_usage')).latest?.id,runtime.latest?.id);
      record('C1 Restart and host configuration boundary','PASS','Connections, deployment history and source opt-in unchanged; runtime observation persisted');
      record('C1 Live installation / login / historical import','SKIPPED','Existing-host observation scope: no component install, login launch, target write, or historical log import was requested by this run');
      record('C2 Clean environment acceptance','BLOCKED','Requires a dedicated user or clean VM; deferred by maintainer. No new user was created.');
      windows.assertClean();
      record('C1 Window boundary across startup, refresh and restart','PASS',{newCodeWindows:windows.added.size});
      return;
    }
    // A separately invoked path, never a fallback from HostObserve. Only a
    // genuinely empty profile can be enrolled; directories are checked, not read.
    if (!enrollDedicatedProfile({profile:process.env.USERPROFILE,appData:process.env.APPDATA,runId:manifest.runId},record)) return;
    if(!gate('InstallPackage',{package:path.basename(manifest.artifact.package),sha256:manifest.artifact.packageSha256,action:'Install the prepared current-user NSIS package in this dedicated profile'}))return;
    const installed=path.join(process.env.LOCALAPPDATA,'PilotWeave','pilotweave.exe');
    if(!manifest.packageInstalled) {
      if(digest(fs.readFileSync(manifest.artifact.package))!==manifest.artifact.packageSha256)throw new Error('Package digest changed');
      const p=spawn(manifest.artifact.package,['/S'],{windowsHide:true,stdio:'ignore'});
      const exit=await new Promise(resolve=>{p.once('exit',resolve);p.once('error',()=>resolve(-1));});
      if(exit!==0||!fs.existsSync(installed)||digest(fs.readFileSync(installed))!==manifest.artifact.sha256)throw new Error('Installed executable failed identity verification');
      manifest.packageInstalled=true;save();
    }
    await start(installed);record('C2 Installed package identity','PASS',{sha256:manifest.artifact.sha256});
    // Install one component, then review the remaining components separately.
    for (const [action, first] of [['InstallFirstComponent',true],['InstallRemainingComponents',false]]) {
      const observation=await driver.ipc('get_installation_status');
      const missing=observation.filter(c=>c.status==='missing').map(c=>c.id);
      if(missing.length) {
        if(first&&manifest.firstComponentInstalled)continue;
        const selected=first?[missing.includes('vscode')?'vscode':missing[0]]:missing;
        const plan=await driver.ipc('preview_install',{componentIds:selected});
        const scope={operations:plan.operations.map(o=>({component:o.componentId,strategy:o.strategy,source:o.source,description:o.description}))};
        if(!gate(action,scope))return;
        const result=await driver.ipc('apply_install_plan',{planId:plan.id});
        const success=result.results.every(r=>r.status==='completedAndVerified');
        record('C2 '+action+' rediscovery',success?'PASS':'BLOCKED',states(result.observations));
        if(!success)return;
        if(first){manifest.firstComponentInstalled=true;save();}
      }
    }
    if(!gate('SignIn','Launch official client sign-in flows. Complete GitHub, MFA/device approval in each official client; no capture or credential input automation.'))return;
    if(!manifest.signInLaunched) {
      const plan=await driver.ipc('preview_login',{surfaces:[],targetAccount:null});
      await driver.ipc('apply_login_plan',{planId:plan.id,confirmed:true});manifest.signInLaunched=true;save();
    }
    if(!gate('AccountConfirmation','Complete official sign-in and verify or confirm matching accounts in the PilotWeave Home UI; then resume.'))return;
    const account=await driver.ipc('get_setup_status');
    record('C2 Account evidence','PASS',{alignment:account.alignment,states:account.accounts.surfaces.map(s=>({surface:s.surface,state:s.state}))});
    if(!['verifiedSameAccount','userConfirmedSameAccount'].includes(account.alignment)) { record('C2 Account completion','BLOCKED','Official account evidence is not aligned; review in the app');return; }
    if(!gate('ProviderSetup','Create the test Connection in PilotWeave, review/apply supported targets, and complete the Copilot app manual provider step in its own UI. This gate never creates a real model request.'))return;
    const setup=await driver.ipc('get_setup_status');
    if(!setup.preferences.connectionId||setup.targets.some(t=>!['inSync','manual','notInstalled'].includes(t.state))||(setup.targets.some(t=>t.state==='manual')&&!setup.manualConfirmed)) {record('C2 Provider verification','BLOCKED','Review the actual deployment statuses in Home');return;}
    if(!gate('GitHubAuthorization','Enter a separate least-privilege GitHub authorization in PilotWeave UI. Do not send the token to the runner.'))return;
    const auth=await driver.ipc('get_github_authorization_status');
    if(auth.state!=='verified'){record('C2 Personal Billing','BLOCKED',{capability:auth.state});return;}
    const now=new Date();const billing=await driver.ipc('refresh_personal_github_usage',{year:now.getUTCFullYear(),month:now.getUTCMonth()+1});
    record('C2 Billing endpoint coverage','PASS',{families:billing.families.map(f=>({family:f.family,capability:f.latest?.status}))});
    if(!gate('UsageSources','In this enrolled fresh profile only, create bounded test activity in a test workspace. Configure VS Code metadata export with captureContent=false and close the CLI session normally. Enable only supported sources in PilotWeave.'))return;
    const sources=await driver.ipc('get_usage_sources');
    if(!sources.some(s=>s.enabled)){record('C2 Usage source opt-in','BLOCKED','No source explicitly enabled');return;}
    await driver.ipc('sync_local_usage',{sourceIds:[]});const one=await driver.ipc('get_usage_overview');
    await driver.ipc('sync_local_usage',{sourceIds:[]});const two=await driver.ipc('get_usage_overview');assert.deepEqual(two.totals,one.totals);
    record('C2 Real source idempotence',two.totalRecords>0?'PASS':'BLOCKED',{records:two.totalRecords,status:two.status});
    await driver.stop();await start(installed);assert.deepEqual((await driver.ipc('get_usage_overview')).totals,two.totals);
    record('C2 Installed application restart','PASS','Native metadata and estimates persisted');
    record('C2 Real paid requests','SKIPPED','Runner never submits model prompts; any independently authorized test requests are completed in official clients');
  } finally { await driver?.stop(); await windows?.stop(); save();record('C Cleanup','PASS','Stopped only this run’s driver/application tree and read-only observer; client data retained'); }
}
