import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { spawn } from 'node:child_process';
import { setTimeout as delay } from 'node:timers/promises';

const pwsh = process.env.PILOTWEAVE_PWSH ?? path.join(process.env.USERPROFILE,
  '.cache/codex-runtimes/codex-primary-runtime/dependencies/native/powershell/pwsh.exe');
const quote = value => "'" + value.replaceAll("'", "''") + "'";
const exited = child => child.exitCode !== null || child.signalCode !== null;

for (const abrupt of [false, true]) test(`Windows job closes descendants after ${abrupt ? 'parent termination' : 'failure/timeout cleanup'}`, { timeout: 20000, skip: process.platform !== 'win32' }, async () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'pilotweave-job-'));
  const childFile = path.join(root, 'child.mjs');
  const marker = path.join(root, 'ready.json');
  const script = path.join(root, 'parent.ps1');
  fs.writeFileSync(childFile, "import fs from 'node:fs'; import http from 'node:http'; const s=http.createServer((q,r)=>r.end('fixture')); s.listen(0,'127.0.0.1',()=>fs.writeFileSync(process.argv[2],JSON.stringify({pid:process.pid,port:s.address().port}))); ");
  fs.writeFileSync(script, [
    "$ErrorActionPreference='Stop'",
    '. ' + quote(path.resolve(import.meta.dirname, '../../../scripts/validation-job.ps1')),
    '$s=[Diagnostics.ProcessStartInfo]::new(' + quote(process.execPath) + ')',
    '$s.UseShellExecute=$false; $s.CreateNoWindow=$true',
    '$s.ArgumentList.Add(' + quote(childFile) + '); $s.ArgumentList.Add(' + quote(marker) + ')',
    '$p=[Diagnostics.Process]::Start($s); $job=[PilotWeaveValidationJob]::Attach($p.Handle)',
    "try { [Console]::WriteLine('ATTACHED'); [Console]::Out.Flush(); [Console]::ReadLine() | Out-Null } finally { [PilotWeaveValidationJob]::CloseHandle($job) | Out-Null }",
  ].join('\n'));
  const parent = spawn(fs.existsSync(pwsh) ? pwsh : 'pwsh.exe', ['-NoProfile', '-File', script], { windowsHide: true, stdio: ['pipe', 'pipe', 'pipe'] });
  let output = '', startupError;
  parent.stdout.on('data', b => { output += b; }); parent.stderr.on('data', b => { output += b; });
  parent.on('error', e => { startupError = e; });
  let fixture;
  try {
    const limit = Date.now() + 10000;
    while ((!output.includes('ATTACHED') || !fs.existsSync(marker)) && Date.now() < limit && !exited(parent) && !startupError) await delay(50);
    if (startupError) throw startupError;
    assert.ok(output.includes('ATTACHED') && fs.existsSync(marker), output);
    fixture = JSON.parse(fs.readFileSync(marker));
    assert.equal((await fetch('http://127.0.0.1:' + fixture.port, { signal: AbortSignal.timeout(1000) })).status, 200);
    if (abrupt) parent.kill(); else parent.stdin.end('\n');
    const deadline = Date.now() + 5000;
    while (Date.now() < deadline) {
      try { process.kill(fixture.pid, 0); await delay(50); } catch { break; }
    }
    assert.throws(() => process.kill(fixture.pid, 0));
    await assert.rejects(fetch('http://127.0.0.1:' + fixture.port, { signal: AbortSignal.timeout(500) }));
  } finally {
    if (!exited(parent)) parent.kill();
    // Only PIDs from this test's private marker, never process-name cleanup.
    if (fixture) { try { process.kill(fixture.pid); } catch {} }
    const absolute = path.resolve(root);
    assert.ok(absolute.startsWith(path.resolve(os.tmpdir()) + path.sep));
    fs.rmSync(absolute, { recursive: true, force: true });
  }
});
