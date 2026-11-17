import { mkdir } from 'node:fs/promises';
import path from 'node:path';
import { CodexRpc } from './codex-rpc.mjs';
import { Bridge } from './bridge.mjs';
import { createApiServer } from './server.mjs';
import { safeError } from './errors.mjs';
import { DEFAULT_MODEL, DEFAULT_REASONING_EFFORT } from './defaults.mjs';

export class Controller {
  constructor({ dataDir, token, port = 18321, binary = 'codex' }) {
    this.dataDir = dataDir; this.token = token; this.port = port; this.binary = binary;
    this.logs = []; this.server = null; this.rpc = null; this.bridge = null;
    this.account = null; this.quota = null; this.modelList = []; this.error = null;
  }

  log(event) {
    this.logs.unshift({ at: new Date().toISOString(), ...event });
    this.logs.length = Math.min(this.logs.length, 100);
  }

  async connect() {
    if (this.rpc?.ready) return;
    const cwd = path.join(this.dataDir, 'workspace');
    await mkdir(cwd, { recursive: true });
    if (this.bridge) this.bridge.close();
    this.rpc = new CodexRpc({ binary: this.binary, cwd });
    await this.rpc.start();
    this.bridge = new Bridge(this.rpc, { cwd, log: e => this.log(e) });
  }

  async refresh() {
    try {
      await this.connect();
      const [account, models, quota] = await Promise.all([
        this.rpc.request('account/read', { refreshToken: false }), this.bridge.models(),
        this.rpc.request('account/rateLimits/read', {}).catch(e => ({ error: safeError(e) })),
      ]);
      // Only return display fields; never expose credentials via IPC.
      this.account = account.account ? { type: account.account.type, planType: account.account.planType || null } : null;
      this.modelList = models.map(m => ({ model: m.model, displayName: m.displayName, isDefault: m.isDefault }));
      this.quota = quota; this.error = null;
    } catch (error) { this.error = safeError(error); }
    return this.status();
  }

  status() {
    return { running: !!this.server?.listening, connected: !!this.rpc?.ready, port: this.port,
      baseUrl: `http://127.0.0.1:${this.port}/v1`, binary: this.binary,
      resolvedBinary: this.rpc?.resolvedBinary || null,
      defaultModel: DEFAULT_MODEL, defaultReasoningEffort: DEFAULT_REASONING_EFFORT,
      account: this.account, quota: this.quota, models: this.modelList,
      activeSessions: this.bridge?.sessions.size || 0, logs: this.logs, error: this.error };
  }

  async start() {
    if (this.server?.listening) return this.status();
    await this.refresh();
    if (this.error) throw new Error(this.error);
    if (this.account?.type !== 'chatgpt') throw new Error('请先在本机 Codex 中使用 ChatGPT 登录，再启动服务。');
    const server = createApiServer({ bridge: this.bridge, token: this.token, log: e => this.log(e) });
    await new Promise((resolve, reject) => {
      server.once('error', reject);
      server.listen(this.port, '127.0.0.1', resolve);
    });
    server.on('error', e => this.log({ type: 'error', message: safeError(e) }));
    this.server = server;
    this.log({ type: 'service', status: 'started' });
    return this.status();
  }

  async stop() {
    const server = this.server; this.server = null;
    this.bridge?.close(); this.bridge = null;
    if (server) {
      const closed = new Promise(resolve => server.close(resolve));
      server.closeAllConnections(); await closed;
    }
    await this.rpc?.stop(); this.rpc = null;
    this.log({ type: 'service', status: 'stopped' });
    return this.status();
  }

  async update({ port, binary }) {
    if (!Number.isInteger(port) || port < 1024 || port > 65535) throw new Error('端口请输入 1024–65535 的整数。');
    if (typeof binary !== 'string' || !binary.trim()) throw new Error('请选择 Codex 可执行程序。');
    if (this.server?.listening) throw new Error('请先停止服务再修改设置。');
    await this.stop();
    this.port = port; this.binary = binary.trim();
    return this.refresh();
  }

  async selfTest() {
    if (!this.server?.listening) throw new Error('请先启动服务。');
    const response = await fetch(`http://127.0.0.1:${this.port}/v1/chat/completions`, {
      method: 'POST', headers: { Authorization: `Bearer ${this.token}`, 'Content-Type': 'application/json' },
      body: JSON.stringify({ model: DEFAULT_MODEL, messages: [{ role: 'user', content: 'Reply with exactly: AI Router OK' }] }),
      signal: AbortSignal.timeout(90000),
    });
    const result = await response.json();
    if (!response.ok) throw new Error(result.error?.message || '连接测试失败。');
    return result.choices[0].message.content;
  }
}
