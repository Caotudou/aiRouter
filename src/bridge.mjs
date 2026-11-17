import { randomUUID } from 'node:crypto';
import { RouterError } from './errors.mjs';
import { DEFAULT_MODEL, DEFAULT_REASONING_EFFORT } from './defaults.mjs';
import { canonical, normalizeMessages, toolsForCodex, userInput, toolOutput, textContent, validateOptions } from './protocol.mjs';

const INSTRUCTIONS = `You are the model backend for WorkBuddy. Respond to the user's messages using the provided conversation context. Tools named wb_tool_* are supplied and executed by WorkBuddy; use them for all external actions. Do not use Codex built-in tools, skills, commands, filesystem access, plans, or subagents. Do not claim an action occurred unless a WorkBuddy tool result confirms it. Return normal assistant text or invoke a provided tool.`;

export class Bridge {
  constructor(rpc, { cwd, maxSessions = 24, ttlMs = 10 * 60 * 1000, timeoutMs = 180000, log = () => {} } = {}) {
    this.rpc = rpc;
    this.cwd = cwd;
    this.maxSessions = maxSessions;
    this.ttlMs = ttlMs;
    this.timeoutMs = timeoutMs;
    this.log = log;
    this.sessions = new Map();
    this.creating = 0;
    this.onNotification = m => this.notification(m);
    this.onServerRequest = m => this.serverRequest(m);
    this.onDisconnect = e => { for (const s of this.sessions.values()) this.drop(s, e); };
    rpc.on('notification', this.onNotification);
    rpc.on('serverRequest', this.onServerRequest);
    rpc.on('disconnected', this.onDisconnect);
    this.sweeper = setInterval(() => {
      for (const s of this.sessions.values()) if (!s.waiter && Date.now() - s.touched > this.ttlMs) this.drop(s);
    }, 15000);
    this.sweeper.unref();
  }

  async models() {
    const models = []; let cursor;
    do {
      const page = await this.rpc.request('model/list', { limit: 100, includeHidden: false, ...(cursor ? { cursor } : {}) });
      models.push(...page.data);
      cursor = page.nextCursor;
    } while (cursor);
    return models;
  }

  async complete(body, { signal, onDelta = () => {}, onStart = () => {} } = {}) {
    validateOptions(body);
    const messages = normalizeMessages(body.messages);
    const toolSpec = toolsForCodex(body.tools, body.tool_choice);
    const models = await this.models();
    const model = body.model === 'codex-local' || !body.model
      ? DEFAULT_MODEL : body.model;
    if (!models.some(m => m.model === model)) throw new RouterError(`当前 Codex 目录中未找到模型：${model}`, 400, 'model_not_found');
    const effort = body.reasoning_effort ?? body.reasoning?.effort ?? DEFAULT_REASONING_EFFORT;
    const descriptor = models.find(m => m.model === model);
    if (effort && !descriptor.supportedReasoningEfforts?.some(e => e.reasoningEffort === effort)) {
      throw new RouterError(`模型不支持推理强度：${effort}`, 400, 'invalid_request');
    }
    const identity = canonical({ model, tools: body.tools || [], choice: body.tool_choice || 'auto' });
    let s = [...this.sessions.values()].filter(s => s.identity === identity &&
      s.history.length <= messages.length && canonical(messages.slice(0, s.history.length)) === canonical(s.history))
      .sort((a, b) => b.history.length - a.history.length)[0];
    if (s?.waiter) throw new RouterError('同一对话已有请求运行中。', 409, 'conversation_busy');
    let incoming = s ? messages.slice(s.history.length) : messages;
    if (s && !incoming.length) throw new RouterError('消息未包含新的输入。', 409, 'duplicate_request');
    if (!s && messages.some(m => m.role === 'tool')) {
      throw new RouterError('工具调用会话已过期、服务已重启或历史已改变，请在 WorkBuddy 新建任务重试。', 409, 'tool_session_expired');
    }
    if (s?.pending.size) {
      const expected = [...s.pending.values()].filter(p => p.reported);
      if (incoming.length !== expected.length || incoming.some(m => m.role !== 'tool') ||
        new Set(incoming.map(m => m.tool_call_id)).size !== incoming.length ||
        incoming.some(m => !s.pending.get(m.tool_call_id)?.reported)) {
        throw new RouterError('请完整回传上一轮所有 tool_calls 的结果。', 400, 'invalid_tool_results');
      }
    } else if (incoming.some(m => m.role === 'tool')) {
      throw new RouterError('没有对应的待处理工具调用。', 400, 'invalid_tool_results');
    }
    if (!s) {
      if (this.sessions.size + this.creating >= this.maxSessions) throw new RouterError('本机对话数已达上限，请稍后重试或重启服务。', 429, 'overloaded');
      this.creating++;
      try {
        const systems = messages.filter(m => ['system', 'developer'].includes(m.role)).map(m => textContent(m.content)).join('\n\n');
        const result = await this.rpc.request('thread/start', {
          model, modelProvider: 'openai', ephemeral: true, cwd: this.cwd, environments: [],
          approvalPolicy: 'untrusted', sandbox: 'read-only',
          baseInstructions: `${INSTRUCTIONS}\n\n${systems}`,
          developerInstructions: 'All computer actions must be delegated to WorkBuddy dynamic tools. Do not use built-in tools.',
          dynamicTools: toolSpec.dynamicTools,
          config: { 'features.shell_tool': false, 'features.multi_agent': false, 'features.apps': false, 'web_search': 'disabled' },
        });
        s = { id: result.thread.id, identity, model, names: toolSpec.names, history: [], pending: new Map(),
          buffer: [], content: '', done: false, touched: Date.now(), usage: null };
        this.sessions.set(s.id, s);
      } finally { this.creating--; }
    }
    if (signal?.aborted) { this.drop(s); throw new RouterError('客户端已取消请求。', 499, 'cancelled'); }
    s.touched = Date.now();
    const id = `chatcmpl-${randomUUID()}`;
    const created = Math.floor(Date.now() / 1000);
    const answer = new Promise((resolve, reject) => {
      const abort = () => this.drop(s, new RouterError('客户端已取消请求。', 499, 'cancelled'));
      const timer = setTimeout(() => this.drop(s, new RouterError('Codex 响应超时，请重试。', 504, 'timeout')), this.timeoutMs);
      signal?.addEventListener('abort', abort, { once: true });
      s.waiter = { resolve, reject, onDelta, messages, id, created, cleanup: () => {
        clearTimeout(timer); signal?.removeEventListener('abort', abort);
      } };
    });
    // Notifications can reject while turn/start itself is still pending.
    answer.catch(() => {});
    try {
      onStart({ id, created, model: s.model });
      // A following tool request can arrive before the next HTTP request.
      for (const delta of s.buffer.splice(0)) onDelta(delta);
      if (s.pending.size) {
        // Validate every result before resolving any Codex server request.
        for (const m of incoming) {
          const p = s.pending.get(m.tool_call_id);
          s.pending.delete(m.tool_call_id);
          this.rpc.reply(p.rpcId, { contentItems: toolOutput(m.content), success: true });
        }
        this.flushTools(s);
        if (s.done && !s.pending.size) this.finish(s);
      } else {
        s.content = ''; s.done = false; s.usage = null;
        const input = userInput(incoming);
        if (!input.length) throw new RouterError('请求没有用户输入。', 400, 'invalid_request');
        const started = await this.rpc.request('turn/start', { threadId: s.id, input, environments: [], ...(effort ? { effort } : {}) });
        s.turnId = started.turn.id;
      }
    } catch (e) { this.drop(s, e); }
    return answer;
  }

  notification({ method, params: p }) {
    const s = this.sessions.get(p?.threadId);
    if (!s) return;
    if (method === 'turn/started') s.turnId = p.turn.id;
    if (method === 'item/agentMessage/delta') {
      s.content += p.delta;
      if (s.waiter) s.waiter.onDelta(p.delta); else s.buffer.push(p.delta);
    }
    if (method === 'thread/tokenUsage/updated') s.usage = p.tokenUsage?.last;
    if (method === 'turn/completed') {
      s.done = true;
      if (p.turn.status !== 'completed') {
        this.drop(s, new RouterError(p.turn.error?.message || `Codex turn ${p.turn.status}`, 502, 'codex_turn_failed'));
      } else this.finish(s);
    }
    // Stop immediately if the agent ever attempts a native action.
    if (method === 'item/started' && ['commandExecution', 'fileChange', 'mcpToolCall', 'collabAgentToolCall'].includes(p.item?.type)) {
      this.drop(s, new RouterError('Codex 尝试使用内置工具，已中断。请让 WorkBuddy 使用其自身工具。', 502, 'native_tool_blocked'));
    }
  }

  serverRequest(message) {
    const { method, params: p, id } = message;
    const s = this.sessions.get(p?.threadId);
    if (method === 'item/tool/call' && s && s.names.has(p.tool)) {
      const callId = `call_${randomUUID().replaceAll('-', '')}`;
      s.pending.set(callId, { rpcId: id, reported: false, call: { id: callId, type: 'function',
        function: { name: s.names.get(p.tool), arguments: typeof p.arguments === 'string' ? p.arguments : JSON.stringify(p.arguments) } } });
      this.flushTools(s);
    } else {
      this.rpc.reject(id, 'AI Router delegates actions only to WorkBuddy tools; this request is unsupported.');
      if (s) this.drop(s, new RouterError(`不支持的 Codex 请求：${method}`, 502, 'unsupported_codex_request'));
    }
  }

  flushTools(s) {
    if (!s.waiter || s.flushTimer || ![...s.pending.values()].some(p => !p.reported)) return;
    s.flushTimer = setTimeout(() => {
      s.flushTimer = null;
      if (!s.waiter) return;
      const calls = [...s.pending.values()].filter(p => !p.reported);
      if (!calls.length) return;
      for (const p of calls) p.reported = true;
      this.finish(s, calls.map(p => p.call));
    }, 35);
  }

  finish(s, toolCalls = []) {
    const w = s.waiter;
    if (!w) return;
    s.waiter = null; w.cleanup();
    const message = { role: 'assistant', content: s.content || null, ...(toolCalls.length ? { tool_calls: toolCalls } : {}) };
    s.history = [...w.messages, message];
    s.content = ''; s.touched = Date.now();
    const result = { id: w.id, object: 'chat.completion', created: w.created, model: s.model,
      choices: [{ index: 0, message, finish_reason: toolCalls.length ? 'tool_calls' : 'stop' }] };
    if (s.usage) result.usage = { prompt_tokens: s.usage.inputTokens, completion_tokens: s.usage.outputTokens,
      total_tokens: s.usage.totalTokens, prompt_tokens_details: { cached_tokens: s.usage.cachedInputTokens } };
    this.log({ type: 'request', status: toolCalls.length ? 'tool_calls' : 'completed', model: s.model });
    w.resolve(result);
  }

  drop(s, error = new RouterError('会话已释放。', 503, 'session_closed')) {
    this.sessions.delete(s.id);
    clearTimeout(s.flushTimer);
    if (s.waiter) { const w = s.waiter; s.waiter = null; w.cleanup(); w.reject(error); }
    for (const p of s.pending.values()) {
      try { this.rpc.reply(p.rpcId, { success: false, contentItems: [{ type: 'inputText', text: 'Session cancelled.' }] }); } catch {}
    }
    s.pending.clear();
    if (s.turnId && !s.done) this.rpc.request('turn/interrupt', { threadId: s.id, turnId: s.turnId }).catch(() => {});
    this.rpc.request('thread/unsubscribe', { threadId: s.id }).catch(() => {});
  }

  close() {
    clearInterval(this.sweeper);
    for (const s of this.sessions.values()) this.drop(s);
    this.rpc.off('notification', this.onNotification);
    this.rpc.off('serverRequest', this.onServerRequest);
    this.rpc.off('disconnected', this.onDisconnect);
  }
}
