import fs from 'node:fs';
import path from 'node:path';
import os from 'node:os';
import { spawn, execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { setTimeout as delay } from 'node:timers/promises';
import { Report, digest, redact } from './validation-report.mjs';

const [mode, runRoot, toolRoot, liveTarget, resume, completedAction] = process.argv.slice(2);
const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const release = process.env.PILOTWEAVE_VALIDATION_RELEASE;
if (release) { const end = Date.now() + 10000; while (!fs.existsSync(release) && Date.now() < end) await delay(25); if (!fs.existsSync(release)) throw new Error('Parent job assignment failed'); }
if (!['Regression','NativeIsolated','PrepareLive','LiveAcceptance'].includes(mode) || !path.isAbsolute(runRoot) || !path.isAbsolute(toolRoot)) throw new Error('Invalid validation invocation');
fs.mkdirSync(runRoot, { recursive: true });
const git = (...args) => execFileSync('git.exe', args, { cwd: repo, encoding: 'utf8', windowsHide: true }).trim();
const treeHash = () => {
  const files = [...new Set(git('ls-files', '-co', '--exclude-standard', '-z').split('\0').filter(Boolean))].sort();
  return digest(files.map(f => f + '\0' + (fs.existsSync(path.join(repo,f)) ? digest(fs.readFileSync(path.join(repo,f))) : 'deleted') + '\n').join(''));
};
const provenance = { git: git('rev-parse','HEAD'), branch: git('branch','--show-current'),
  treeSha256: treeHash(),
  dirty: Boolean(git('status','--porcelain')), platform: `${os.platform()} ${os.release()} ${os.arch()}`, node: process.version };
const previous = path.join(runRoot, 'summary.json');
if (fs.existsSync(previous)) {
  // Never overwrite previous failures with a resumed/retried result.
  const history = path.join(runRoot, 'attempts', String(Date.now())); fs.mkdirSync(history, { recursive: true });
  for (const name of ['summary.json','report.html','junit.xml']) if (fs.existsSync(path.join(runRoot,name))) fs.copyFileSync(path.join(runRoot,name),path.join(history,name));
}
const report = new Report(runRoot, mode, provenance);
const record = report.record.bind(report);
const privateRoot = path.join(runRoot, 'private'); fs.mkdirSync(privateRoot, { recursive: true });
const deadline = setTimeout(() => {
  record('Run deadline','FAIL','Run exceeded 45 minutes; parent Job Object will terminate all owned descendants');
  report.data.finishedAt=new Date().toISOString();report.save();process.exit(1);
},45*60*1000);
async function ensureDependencies() {
  if (!fs.existsSync(path.join(repo,'node_modules/@tauri-apps/cli/tauri.js'))) {
    await command('Install locked build tooling','cmd.exe',['/d','/s','/c','npm.cmd ci --ignore-scripts']);
  }
  const installed=path.join(repo,'tests/native-e2e/node_modules/webdriverio/package.json');
  const pinned=JSON.parse(fs.readFileSync(path.join(repo,'tests/native-e2e/package.json'))).devDependencies.webdriverio;
  if(!fs.existsSync(installed)||JSON.parse(fs.readFileSync(installed)).version!==pinned) {
    await command('Install locked native tooling','cmd.exe',['/d','/s','/c','npm.cmd ci --prefix tests/native-e2e --ignore-scripts']);
  }
  if(JSON.parse(fs.readFileSync(installed)).version!==pinned)throw new Error('WebdriverIO version mismatch');
  record('Locked native tooling','PASS',{webdriverio:pinned});
}
async function command(name, executable, args, { timeout = 1200000, env = {}, allowFailure = false } = {}) {
  const started = Date.now(); let output = '', overflow = false, timedOut = false;
  const child = spawn(executable, args, { cwd: repo, env: { ...process.env, ...env }, windowsHide: true, stdio: ['ignore','pipe','pipe'] });
  for (const stream of [child.stdout, child.stderr]) stream.on('data', chunk => { if (output.length < 2 * 1024 * 1024) output += chunk.toString(); else overflow = true; });
  const timer = setTimeout(() => { timedOut = true; spawn('taskkill.exe',['/PID',String(child.pid),'/T','/F'],{windowsHide:true,stdio:'ignore'}); }, timeout);
  const heartbeat = setInterval(() => process.stdout.write(`RUNNING ${name}\n`), 45000);
  const code = await new Promise(resolve => { child.once('error', e => { output += e.message; resolve(-1); }); child.once('exit', resolve); });
  clearTimeout(timer); clearInterval(heartbeat);
  const success = code === 0 && !timedOut && !overflow;
  fs.writeFileSync(path.join(runRoot, name.replace(/[^a-z0-9-]/gi,'_') + '.log'), redact(output));
  if (!allowFailure) { record(name, success ? 'PASS' : 'FAIL', { exitCode: code, timedOut, outputTruncated: overflow }, Date.now()-started); if (!success) throw new Error(`${name} failed; see its redacted log`); }
  return { code, output, success };
}
const node = (name, file, args = [], extra) => command(name, process.execPath, [file, ...args], extra);
const tauri = (name, args) => node(name, 'node_modules/@tauri-apps/cli/tauri.js', ['build','--config','apps/desktop/src-tauri/tauri.conf.json', ...args]);
const cargo = (name, args) => command(name, 'cargo.exe', args);
async function buildNative() {
  await tauri('Build isolated native executable', ['--no-bundle','--features','local-e2e','--config','apps/desktop/src-tauri/tauri.local-e2e.conf.json']);
  const dest = path.join(runRoot,'artifacts','isolated'); fs.mkdirSync(dest,{recursive:true});
  const application = path.join(dest,'pilotweave.exe'); fs.copyFileSync(path.join(repo,'target/release/pilotweave.exe'),application);
  record('Isolated artifact identity','PASS',{ features: ['local-e2e'], sha256: digest(fs.readFileSync(application)) }); return application;
}
async function isolatedGuards(application) {
  const invalid=path.join(privateRoot,'invalid-context');fs.mkdirSync(invalid,{recursive:true});
  for(const [name,args] of [['without context',[]],['unowned root',['--local-e2e-root='+invalid]],['parent traversal',['--local-e2e-root='+invalid+'/..']]]) {
    const result=await command('Isolated rejects '+name,application,args,{timeout:10000,allowFailure:true});
    if(result.code!==2)throw new Error('Isolated build failed closed-context check: '+name);
  }
  const junction=path.join(privateRoot,'junction-root');
  fs.symlinkSync(invalid,junction,'junction');
  try {
    const result=await command('Isolated rejects reparse root',application,['--local-e2e-root='+junction],{timeout:10000,allowFailure:true});
    if(result.code!==2)throw new Error('Isolated build accepted a reparse root');
  } finally {fs.unlinkSync(junction);}
  if(fs.existsSync(path.join(invalid,'config')))throw new Error('Rejected context still opened stores');
  record('B00 Isolation rejects missing, unowned, parent and reparse roots','PASS','All rejected with exit 2 before opening stores');
}
async function defaultBuild(packageBuild = false) {
  await tauri(packageBuild ? 'Default NSIS package build' : 'Default release build', packageBuild ? ['--config','apps/desktop/src-tauri/tauri.package.conf.json','--bundles','nsis'] : ['--no-bundle']);
  const dest = path.join(runRoot,'artifacts','default'); fs.mkdirSync(dest,{recursive:true});
  const application = path.join(dest,'pilotweave.exe'); fs.copyFileSync(path.join(repo,'target/release/pilotweave.exe'),application);
  const artifact = { application, sha256: digest(fs.readFileSync(application)), features: 'default', ...provenance };
  if (packageBuild) {
    const directory = path.join(repo,'target/release/bundle/nsis'); const packages = fs.readdirSync(directory).filter(n => n.endsWith('.exe'));
    if (packages.length !== 1) throw new Error('Expected one unambiguous NSIS package');
    artifact.package = path.join(dest,packages[0]); fs.copyFileSync(path.join(directory,packages[0]),artifact.package);
    artifact.packageSha256 = digest(fs.readFileSync(artifact.package));
  }
  fs.writeFileSync(path.join(dest,'artifact.json'),JSON.stringify(artifact,null,2));
  record('Default artifact identity','PASS',{sha256:artifact.sha256,packageSha256:artifact.packageSha256,features:'default'});
  const negative = await command('Production rejects isolated root', application, ['--local-e2e-root',path.join(privateRoot,'rejected')], { timeout:10000,allowFailure:true });
  if (negative.code !== 2 || fs.existsSync(path.join(privateRoot,'rejected'))) throw new Error('Production accepted an isolated test argument');
  record('Production rejects isolated root','PASS','Rejected before opening native stores');
  return artifact;
}
async function drivers() {
  const runtimeDir = path.join(process.env['ProgramFiles(x86)'] ?? 'C:/Program Files (x86)','Microsoft/EdgeWebView/Application');
  const versions = fs.existsSync(runtimeDir) ? fs.readdirSync(runtimeDir).filter(n => /^\d+\.\d+\.\d+\.\d+$/.test(n)) : [];
  versions.sort((a,b) => { const aa=a.split('.').map(Number),bb=b.split('.').map(Number); for(let i=0;i<4;i++) if(aa[i]!==bb[i])return bb[i]-aa[i]; return 0; });
  if (!versions.length) throw new Error('BLOCKED_DRIVER: WebView2 runtime not detected');
  const version = versions[0]; const directory = path.join(toolRoot,`msedgedriver-${version}`); const edgeDriver = path.join(directory,'msedgedriver.exe');
  if (!fs.existsSync(edgeDriver)) {
    fs.mkdirSync(directory,{recursive:true}); const response=await fetch(`https://msedgedriver.microsoft.com/${version}/edgedriver_win64.zip`,{signal:AbortSignal.timeout(60000),redirect:'error'});
    if(!response.ok)throw new Error('BLOCKED_DRIVER: matching Edge driver unavailable');
    const chunks=[];let received=0;
    for await (const chunk of response.body) {
      received+=chunk.length;if(received>50*1024*1024)throw new Error('Driver archive exceeds limit');chunks.push(chunk);
    }
    const bytes=Buffer.concat(chunks);
    const zip=path.join(directory,'driver.zip');fs.writeFileSync(zip,bytes);
    const ps="Expand-Archive -LiteralPath '"+zip.replaceAll("'","''")+"' -DestinationPath '"+directory.replaceAll("'","''")+"' -Force";
    await command('Expand matching Edge driver','powershell.exe',['-NoProfile','-NonInteractive','-Command',ps]);
  }
  const driverVersion=execFileSync(edgeDriver,['--version'],{encoding:'utf8',windowsHide:true}).trim();
  if(!driverVersion.includes(version))throw new Error('BLOCKED_DRIVER: Edge driver version mismatch');
  record('Pinned external drivers','PASS',{transport:'Direct Edge WebDriver / WebView2',webviewDirectoryVersion:version,edgeDriver:driverVersion,sha256:digest(fs.readFileSync(edgeDriver))});
  return {edgeDriver};
}
async function regression() {
  await command('Web checks','cmd.exe',['/d','/s','/c','npm.cmd run check:web']);
  await cargo('Rust formatting',['fmt','--all','--','--check']);
  await cargo('Rust default tests',['test','--workspace']);
  await cargo('Rust isolated feature tests',['test','--workspace','--features','local-e2e']);
  await cargo('Rust clippy default',['clippy','--workspace','--all-targets','--','-D','warnings']);
  await cargo('Rust clippy isolated',['clippy','--workspace','--all-targets','--features','local-e2e','--','-D','warnings']);
  await node('Validation tooling tests','--test',['tests/native-e2e/tests/*.test.mjs']);
  const playwright=process.env.PILOTWEAVE_PLAYWRIGHT ?? path.join(process.env.USERPROFILE,'.cache/codex-runtimes/codex-primary-runtime/dependencies/node/node_modules/playwright');
  if(!fs.existsSync(playwright)) throw new Error('Browser fixture runner requires PILOTWEAVE_PLAYWRIGHT');
  await node('Browser mocked contract suite','apps/desktop/web/tests/browser-smoke.cjs',[],{env:{PILOTWEAVE_PLAYWRIGHT:playwright,PILOTWEAVE_BROWSER:path.join(process.env['ProgramFiles(x86)'],'Microsoft/Edge/Application/msedge.exe')}});
  await defaultBuild();
  await command('Git diff whitespace','git.exe',['diff','--check']);
  const tracked=git('ls-files').split('\n');
  if(tracked.some(p=>/(?:\.sqlite3(?:-wal|-shm)?|\.log|\.exe|\.msi)$/.test(p)))throw new Error('Generated/sensitive outputs present in tracked scope');
  record('Tracked artifact boundary','PASS','No database, executable, installer or log tracked');
}
try {
  report.save();
  if(Number(process.versions.node.split('.')[0])<24)throw new Error('Node.js 24 or later is required');
  await ensureDependencies();
  if(mode==='Regression') await regression();
  else if(mode==='NativeIsolated') {
    const application=await buildNative();await isolatedGuards(application);const driverOptions=await drivers();
    const {nativeSuite}=await import('../tests/native-e2e/suite.mjs');
    await nativeSuite({repo,privateRoot,evidenceRoot:runRoot,application,...driverOptions},record);
  } else if(mode==='PrepareLive') {
    const artifact=await defaultBuild(true);const driverOptions=await drivers();
    const manifest={version:1,runId:path.basename(runRoot),target:liveTarget,artifact,driverOptions,preparedAt:new Date().toISOString(),
      scope:liveTarget==='HostObserve'?['default-exe','detection','account-observation','public-prices','readonly-runtime','restart']:['package-install','component-install','official-login','reviewed-deployment','separate-github-auth','fresh-user-import','restart'],
      completed:[],pendingAction:null};
    fs.writeFileSync(path.join(runRoot,'live-manifest.json'),JSON.stringify(manifest,null,2));
    record('Reviewable live manifest','PASS',{target:liveTarget,scope:manifest.scope,requiresResume:true});
  } else {
    if(resume!=='True' && resume!=='true')throw new Error('LiveAcceptance requires -Resume and a prepared RunId');
    const manifest=JSON.parse(fs.readFileSync(path.join(runRoot,'live-manifest.json'),'utf8'));
    if(manifest.runId!==path.basename(runRoot)||manifest.target!==liveTarget)throw new Error('Live target/run differs from reviewed manifest');
    if(digest(fs.readFileSync(manifest.artifact.application))!==manifest.artifact.sha256)throw new Error('Prepared EXE hash changed');
    const {liveAcceptance}=await import('../tests/native-e2e/live.mjs');
    await liveAcceptance({repo,runRoot,manifest,completedAction},record);
  }
} catch(error) { record('Run completion',String(error.message).startsWith('BLOCKED')?'BLOCKED':'FAIL',redact(error.message)); }
finally {
  clearTimeout(deadline);
  const finalTree=treeHash();
  record('Source tree remained unchanged',finalTree===provenance.treeSha256?'PASS':'FAIL',{started:provenance.treeSha256,finished:finalTree});
  report.data.finishedAt=new Date().toISOString();report.save();
}
process.exitCode=report.data.outcome==='FAIL'?1:report.data.outcome==='BLOCKED'?2:0;
