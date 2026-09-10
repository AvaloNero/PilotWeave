import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { Report, redact } from '../../../scripts/validation-report.mjs';
import { acceptGate, enrollDedicatedProfile } from '../live.mjs';
import { addedCodeWindows } from '../host-windows.mjs';

test('window guard ignores existing windows and detects new windows even in an existing process',()=>{
  const before=['code:10:100','pilotweave:20:200'];
  assert.deepEqual(addedCodeWindows(before,['code:10:100','pilotweave:21:201']),[]);
  assert.deepEqual(addedCodeWindows(before,['code:10:101','code - insiders:30:300']),['code:10:101','code - insiders:30:300']);
});

test('report escapes hostile fields and does not count blockers as passes',()=>{
  const root=fs.mkdtempSync(path.join(os.tmpdir(),'pilotweave-report-'));
  try {const report=new Report(root,'<script>',{git:'fixture'});report.record('check','BLOCKED','<img src=x onerror=alert(1)>');
    assert.equal(JSON.parse(fs.readFileSync(path.join(root,'summary.json'))).outcome,'BLOCKED');
    assert.match(fs.readFileSync(path.join(root,'junit.xml'),'utf8'),/<skipped/);
    assert.doesNotMatch(fs.readFileSync(path.join(root,'report.html'),'utf8'),/<img/);
    report.record('failure','FAIL','ghp_abcdefghijklmno');assert.equal(report.data.outcome,'FAIL');
    assert.doesNotMatch(fs.readFileSync(path.join(root,'summary.json'),'utf8'),/ghp_abcdefghijklmno/);
  } finally {fs.rmSync(root,{recursive:true});}
});
test('live gates require the exact pending action and never infer approval from resume',()=>{
  const manifest={completed:[],pendingAction:null};
  assert.equal(acceptGate(manifest,'SignIn','SignIn','review'),false);
  assert.equal(acceptGate(manifest,'SignIn','InstallPackage','review'),false);
  assert.equal(acceptGate(manifest,'SignIn',undefined,'review'),false);
  assert.equal(acceptGate(manifest,'SignIn','SignIn','review'),true);
  assert.deepEqual(manifest.completed,['SignIn']);
});
test('credential patterns are redacted before evidence persistence',()=>{
  for(const key of ['github_pat_abcdefghijklmnop','sk-abcdefghijklmnop','ghp_abcdefghijklmnop'])assert.ok(!redact(key).includes(key));
});

test('unfinished report is running and nested sensitive detail is redacted',()=>{
  const root=fs.mkdtempSync(path.join(os.tmpdir(),'pilotweave-report-'));
  try {
    const report=new Report(root,'fixture',{});report.record('one','PASS',{nested:{key:'github_pat_abcdefghijklmnop'}});
    assert.equal(report.data.outcome,'RUNNING');assert.doesNotMatch(fs.readFileSync(path.join(root,'summary.json'),'utf8'),/github_pat_/);
    report.data.finishedAt=new Date().toISOString();report.save();assert.equal(report.data.outcome,'PASS');
  } finally {fs.rmSync(root,{recursive:true});}
});

test('price fixture multiplies decimal strings without floating point or exponent notation',async()=>{
  const {fixtures}=await import('../fixture-server.mjs');
  const server=await fixtures(path.resolve(import.meta.dirname,'../../..'));
  try {
    server.scenario.multiplier=2;
    const response=await fetch('http://127.0.0.1:'+server.port+'/prices');
    const value=await response.json();const model=value.data.find(m=>m.id==='openai/gpt-5');
    assert.equal(model.pricing.input_cache_read,'0.000000250');
    assert.doesNotMatch(JSON.stringify(value),/\d[eE][+-]\d/);
  } finally {await server.close();}
});

test('approval is invalidated when the reviewed installation scope changes',()=>{
  const m={completed:[],pendingAction:null};
  assert.equal(acceptGate(m,'InstallFirstComponent',undefined,{ids:['vscode']}),false);
  assert.equal(acceptGate(m,'InstallFirstComponent','InstallFirstComponent',{ids:['copilot-cli']}),false);
  assert.equal(acceptGate(m,'InstallFirstComponent','InstallFirstComponent',{ids:['copilot-cli']}),true);
  assert.equal(acceptGate(m,'InstallFirstComponent',undefined,{ids:['vscode']}),false);
});

function dedicatedFixture() {
  const root=fs.mkdtempSync(path.join(os.tmpdir(),'pilotweave-enrollment-'));
  const cleanup=()=>{assert.equal(path.dirname(path.resolve(root)),path.resolve(os.tmpdir()));assert.ok(path.basename(root).startsWith('pilotweave-enrollment-'));fs.rmSync(root,{recursive:true});};
  return {root,cleanup,marker:path.join(root,'.pilotweave-validation-user.json'),options:{profile:root,appData:path.join(root,'AppData'),runId:'fixture-run',readSid:()=> '"fixture-user","S-1-5-21-100-200-300-1001"'}};
}
test('dedicated enrollment fails closed on missing, malformed or unavailable SID',()=>{
  const f=dedicatedFixture();
  try {
    for (const readSid of [()=>'',()=> '"fixture","S-1-5-"',()=> '"S-1-5-21-100","not-a-sid"',()=>{throw new Error('fixture identity failure');}]) {
      const records=[];
      assert.equal(enrollDedicatedProfile({...f.options,readSid},(...r)=>records.push(r)),false);
      assert.equal(records[0][1],'BLOCKED');assert.match(records[0][2],/SID/);
      assert.equal(fs.existsSync(f.marker),false);
    }
  } finally {f.cleanup();}
});
test('dedicated enrollment preserves ownership and resumes only the original run',()=>{
  const f=dedicatedFixture();
  try {
    assert.equal(enrollDedicatedProfile(f.options,()=>assert.fail('fresh enrollment blocked')),true);
    const original=fs.readFileSync(f.marker,'utf8');fs.mkdirSync(path.join(f.root,'.copilot'));
    for (const changed of [{runId:'another-run'},{readSid:()=> '"other-user","S-1-5-21-100-200-300-1002"'}]) {
      const records=[];
      assert.equal(enrollDedicatedProfile({...f.options,...changed},(...r)=>records.push(r)),false);
      assert.equal(records[0][1],'BLOCKED');assert.match(records[0][2],/Resume the original RunId/);
      assert.equal(fs.readFileSync(f.marker,'utf8'),original);
    }
    assert.equal(enrollDedicatedProfile(f.options,()=>assert.fail('original resume blocked')),true);
    assert.equal(fs.readFileSync(f.marker,'utf8'),original);
  } finally {f.cleanup();}
});
test('dedicated enrollment blocks malformed markers and existing client data',()=>{
  const f=dedicatedFixture();
  try {
    fs.mkdirSync(path.join(f.root,'.copilot'));
    assert.equal(enrollDedicatedProfile(f.options,(_,status)=>assert.equal(status,'BLOCKED')),false);
    assert.equal(fs.existsSync(f.marker),false);
    for (const contents of ['{invalid','null',JSON.stringify({version:2}), 'x'.repeat(1025)]) {
      fs.writeFileSync(f.marker,contents);
      assert.equal(enrollDedicatedProfile(f.options,(_,status)=>assert.equal(status,'BLOCKED')),false);
      assert.equal(fs.readFileSync(f.marker,'utf8'),contents);
    }
  } finally {f.cleanup();}
});
