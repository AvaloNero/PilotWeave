import { remote } from 'webdriverio';
import { spawn } from 'node:child_process';
import net from 'node:net';
import { setTimeout as delay } from 'node:timers/promises';

export async function freePort() {
  const s = net.createServer();
  await new Promise((resolve, reject) => { s.once('error', reject); s.listen(0, '127.0.0.1', resolve); });
  const port = s.address().port;
  await new Promise(resolve => s.close(resolve));
  return port;
}

export async function until(check, message, timeout = 15000) {
  const end = Date.now() + timeout;
  while (Date.now() < end) { if (await check()) return; await delay(100); }
  throw new Error(message);
}

export class NativeDriver {
  constructor({ application, edgeDriver, root }) {
    Object.assign(this, { application, edgeDriver, root });
  }
  async start() {
    this.port = await freePort();
    this.process = spawn(this.edgeDriver, ['--port=' + this.port, '--host=127.0.0.1', '--allowed-ips=127.0.0.1'],
    { windowsHide: true, stdio: 'ignore' });
    this.process.on('error', e => { this.startError = e; });
    await until(async () => {
      if (this.startError) throw this.startError;
      if (this.process.exitCode !== null) throw new Error('Native driver exited before readiness');
      try { const r = await fetch(`http://127.0.0.1:${this.port}/status`, {signal:AbortSignal.timeout(500)}); return r.ok; } catch { return false; }
    }, 'BLOCKED_DRIVER: driver not ready');
    this.browser = await remote({ hostname: '127.0.0.1', port: this.port, logLevel: 'silent',
      connectionRetryCount: 0, connectionRetryTimeout: 45000,
      capabilities: { browserName: 'webview2', 'ms:edgeChromium': true,
        'ms:edgeOptions': { binary: this.application, args: this.root ? ['--local-e2e-root=' + this.root] : [] } } });
    await this.browser.setTimeout({ script: 100000, pageLoad: 30000, implicit: 0 });
    // Edge WebDriver initializes the attached WebView at about:blank. Navigate
    // only to the application's compiled asset protocol, never to an HTTP mock.
    this.initialUrl = await this.browser.getUrl();
    if (this.initialUrl === 'about:blank') await this.browser.url('http://tauri.localhost');
    try {
      await until(() => this.browser.execute(() => Boolean(document.querySelector('#primary-nav') && location.origin !== 'null' && window.__TAURI__?.core?.invoke)), 'Native IPC not ready');
    } catch (e) {
      const state = await this.browser.execute(() => ({ url:location.href, origin:location.origin, ready:document.readyState, nav:!!document.querySelector('#primary-nav') }));
      throw new Error(e.message + ': ' + JSON.stringify(state));
    }
    return this;
  }
  async ipc(command, args = {}) {
    const result = await this.browser.executeAsync((name, args, done) => {
      window.__TAURI__.core.invoke(name, args).then(value => done({ ok: true, value }),
        error => done({ ok: false, error: String(error) }));
    }, command, args);
    if (!result.ok) throw new Error(result.error);
    return result.value;
  }
  async click(selector) {
    const el = await this.browser.$(selector);
    await el.waitForClickable({ timeout: 15000 });
    await el.click();
  }
  async stop() {
    if (this.browser) { try { await this.browser.deleteSession(); } catch {} this.browser = undefined; }
    if (this.process?.pid && this.process.exitCode === null) {
      // Only this newly started driver's job and descendants. No name-based kills.
      const killer = spawn('taskkill.exe', ['/PID', String(this.process.pid), '/T', '/F'], { windowsHide: true, stdio: 'ignore' });
      await new Promise(resolve => { killer.once('exit', resolve); killer.once('error', resolve); });
    }
    if (this.port) await until(async () => {
      try { await fetch(`http://127.0.0.1:${this.port}/status`, { signal: AbortSignal.timeout(300) }); return false; } catch { return true; }
    }, 'Driver port leaked after shutdown', 5000);
  }
}
