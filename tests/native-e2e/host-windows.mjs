import assert from 'node:assert/strict';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawn } from 'node:child_process';
import { setTimeout as delay } from 'node:timers/promises';
import { until } from './driver.mjs';

export function addedCodeWindows(baseline, current) {
  return current.filter(value => /^code(?: - insiders)?:\d+:\d+$/.test(value) && !baseline.includes(value));
}

async function stopOwned(child) {
  if (child?.pid && child.exitCode === null) {
    const killer = spawn('taskkill.exe', ['/PID', String(child.pid), '/T', '/F'], { windowsHide: true, stdio: 'ignore' });
    await new Promise(resolve => { killer.once('exit', resolve); killer.once('error', resolve); });
  }
}

export class HostWindowWatch {
  async start() {
    this.current = [];
    this.added = new Set();
    const script = fileURLToPath(new URL('../../scripts/observe-client-windows.ps1', import.meta.url));
    this.process = spawn('pwsh.exe', ['-NoProfile', '-NonInteractive', '-File', script], { windowsHide: true, stdio: ['ignore', 'pipe', 'ignore'] });
    this.process.on('error', () => { this.failed = true; });
    let pending = '';
    this.process.stdout.on('data', chunk => {
      pending += chunk.toString();
      if (pending.length > 65536) { this.failed = true; return; }
      let newline;
      while ((newline = pending.indexOf('\n')) >= 0) {
        const line = pending.slice(0, newline); pending = pending.slice(newline + 1);
        try {
          const windows = JSON.parse(line);
          if (!Array.isArray(windows) || windows.length > 4096 || !windows.every(w => /^(?:code(?: - insiders)?|pilotweave):\d+:\d+$/.test(w))) throw new Error();
          this.current = windows;
          this.baseline ??= windows;
          for (const w of addedCodeWindows(this.baseline, windows)) this.added.add(w);
        } catch { this.failed = true; }
      }
    });
    await until(() => {
      if (this.failed || this.process.exitCode !== null) throw new Error('Window observer failed');
      return Boolean(this.baseline);
    }, 'Window observer did not initialize', 20000);
    return this;
  }
  assertClean() {
    assert.equal(this.failed || this.process.exitCode !== null, false, 'Window observer must remain available');
    assert.equal(this.added.size, 0, 'VS Code GUI windows opened during read-only startup/refresh');
  }
  async normalStartup(application) {
    assert.ok(path.isAbsolute(application));
    const child = spawn(application, [], { windowsHide: false, stdio: 'ignore' });
    let failed = false; child.on('error', () => { failed = true; });
    try {
      await until(() => {
        if (failed || child.exitCode !== null) throw new Error('Default application exited during normal startup');
        this.assertClean();
        return this.current.some(w => w.startsWith(`pilotweave:${child.pid}:`));
      }, 'Normal application did not show a window', 20000);
      // Observe the real desktop entry point, without WebDriver navigation.
      for (let i = 0; i < 50; i++) { await delay(100); this.assertClean(); }
    } finally { await stopOwned(child); }
  }
  async stop() { await stopOwned(this.process); }
}
