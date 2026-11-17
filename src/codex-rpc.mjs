import { spawn } from 'node:child_process';
import { createInterface } from 'node:readline';
import { EventEmitter } from 'node:events';
import { RouterError } from './errors.mjs';
import { resolveCodexBinary } from './codex-binary.mjs';

// Stdio keeps account credentials inside the installed Codex process.
export class CodexRpc extends EventEmitter {
  constructor({ binary = 'codex', cwd, timeout = 30000 } = {}) {
    super();
    this.binary = binary;
    this.cwd = cwd;
    this.timeout = timeout;
    this.pending = new Map();
    this.nextId = 0;
    this.child = null;
    this.ready = false;
  }

  async start() {
    if (this.ready) return;
    if (this.starting) return this.starting;
    this.starting = this.connect();
    try { await this.starting; } finally { this.starting = null; }
  }

  async connect() {
    this.resolvedBinary = await resolveCodexBinary(this.binary);
    const child = spawn(this.resolvedBinary, ['app-server', '--listen', 'stdio://', '-c', 'mcp_servers={}'], {
      cwd: this.cwd, windowsHide: true, stdio: ['pipe', 'pipe', 'pipe'], shell: false,
    });
    this.child = child;
    child.stderr.on('data', () => {}); // Never persist potentially sensitive Codex diagnostics.
    child.stdin.on('error', () => {});
    child.on('error', (e) => this.fail(new RouterError(`无法启动 Codex：${e.message}。请选择 codex.exe。`, 503)));
    child.on('exit', () => {
      if (this.child === child) this.child = null;
      this.fail(new RouterError('Codex 服务进程已退出，请重新启动服务。', 503));
    });
    createInterface({ input: child.stdout }).on('line', (line) => {
      let message;
      try { message = JSON.parse(line); } catch { return; }
      if (message.method) {
        this.emit(message.id === undefined ? 'notification' : 'serverRequest', message);
      } else if (this.pending.has(message.id)) {
        const p = this.pending.get(message.id);
        this.pending.delete(message.id);
        clearTimeout(p.timer);
        if (message.error) p.reject(new RouterError(message.error.message, 502, 'codex_rpc_error'));
        else p.resolve(message.result);
      }
    });
    try {
      await this.request('initialize', {
        clientInfo: { name: 'ai_router', title: 'AI Router', version: '0.1.2' },
        capabilities: { experimentalApi: true },
      });
      this.send({ method: 'initialized' });
      this.ready = true;
    } catch (e) { await this.stop(); throw e; }
  }

  send(message) {
    if (!this.child?.stdin?.writable) throw new RouterError('Codex 尚未连接。', 503);
    this.child.stdin.write(JSON.stringify(message) + '\n');
  }

  request(method, params = {}) {
    return new Promise((resolve, reject) => {
      const id = ++this.nextId;
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new RouterError(`Codex 请求超时：${method}`, 504, 'timeout'));
      }, this.timeout);
      this.pending.set(id, { resolve, reject, timer });
      try { this.send({ id, method, params }); }
      catch (e) { clearTimeout(timer); this.pending.delete(id); reject(e); }
    });
  }

  reply(id, result) { this.send({ id, result }); }
  reject(id, message) { this.send({ id, error: { code: -32601, message } }); }

  fail(error) {
    this.ready = false;
    for (const p of this.pending.values()) { clearTimeout(p.timer); p.reject(error); }
    this.pending.clear();
    this.emit('disconnected', error);
  }

  async stop() {
    const child = this.child;
    if (!child) return;
    this.child = null;
    this.ready = false;
    child.stdin.end();
    await new Promise((resolve) => {
      const timer = setTimeout(() => { child.kill(); resolve(); }, 1500);
      child.once('exit', () => { clearTimeout(timer); resolve(); });
    });
  }
}
