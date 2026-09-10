import fs from 'node:fs';
import path from 'node:path';
import { createHash } from 'node:crypto';

export const digest = data => createHash('sha256').update(data).digest('hex');
export function redact(value) {
  return String(value).replace(/(?:github_pat_|gh[pousr]_|sk-)[A-Za-z0-9_-]{8,}/g, '[REDACTED]')
    .replace(/(Authorization\s*[:=]\s*(?:Bearer\s+)?)[^\s,;"}]+/gi, '$1[REDACTED]')
    .replaceAll(process.env.USERPROFILE ?? '___NO_HOME___', '<USERPROFILE>');
}
export const escape = text => String(text).replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
export class Report {
  constructor(root, mode, provenance) { this.root = root; this.data = { schemaVersion: 1, mode, startedAt: new Date().toISOString(), provenance, results: [] }; }
  record(name, status, detail, durationMs = 0) {
    if (!['PASS', 'FAIL', 'BLOCKED', 'SKIPPED'].includes(status)) throw new Error('Unknown result status');
    this.data.results.push({ name, status, detail: typeof detail === 'string' ? redact(detail) : JSON.parse(redact(JSON.stringify(detail))), durationMs }); this.save();
    process.stdout.write(`${status} ${name}\n`);
  }
  save() {
    const items = this.data.results;
    this.data.outcome = items.some(r => r.status === 'FAIL') ? 'FAIL' : items.some(r => r.status === 'BLOCKED') ? 'BLOCKED' : this.data.finishedAt ? 'PASS' : 'RUNNING';
    const json = JSON.stringify(this.data, null, 2);
    fs.writeFileSync(path.join(this.root, 'summary.json'), json);
    const html = `<!doctype html><meta charset="utf-8"><title>PilotWeave local validation</title><style>body{font:16px system-ui;margin:40px;max-width:1100px;background:#111820;color:#e7edf3}td,th{padding:12px;text-align:left;border-bottom:1px solid #3b4a58}pre{white-space:pre-wrap;overflow-wrap:anywhere}.PASS{color:#8edea8}.FAIL{color:#ff9090}.BLOCKED{color:#ffd782}small{color:#bbc4cd}</style><h1>${escape(this.data.mode)} · ${this.data.outcome}</h1><p>Actual result and external capability are reported separately. BLOCKED/SKIPPED are not passes.</p><small>${escape(this.data.startedAt)}</small><pre>${escape(JSON.stringify(this.data.provenance, null, 2))}</pre><table><tr><th>Check</th><th>Result</th><th>Evidence / capability</th></tr>${items.map(r => `<tr><td>${escape(r.name)}</td><td class="${r.status}">${r.status}</td><td><pre>${escape(typeof r.detail === 'string' ? r.detail : JSON.stringify(r.detail, null, 2))}</pre></td></tr>`).join('')}</table>`;
    fs.writeFileSync(path.join(this.root, 'report.html'), html);
    fs.writeFileSync(path.join(this.root, 'junit.xml'), `<?xml version="1.0" encoding="UTF-8"?><testsuite name="${escape(this.data.mode)}" tests="${items.length}" failures="${items.filter(r => r.status === 'FAIL').length}" skipped="${items.filter(r => ['BLOCKED','SKIPPED'].includes(r.status)).length}">${items.map(r => `<testcase name="${escape(r.name)}" time="${r.durationMs / 1000}">${r.status === 'FAIL' ? `<failure>${escape(JSON.stringify(r.detail))}</failure>` : ['BLOCKED','SKIPPED'].includes(r.status) ? `<skipped message="${r.status}">${escape(JSON.stringify(r.detail))}</skipped>` : ''}</testcase>`).join('')}</testsuite>`);
  }
}
