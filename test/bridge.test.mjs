import { test } from 'node:test';
import assert from 'node:assert/strict';
import { EventEmitter } from 'node:events';
import { Bridge } from '../src/bridge.mjs';
import { normalizeMessages, toolsForCodex } from '../src/protocol.mjs';
import { DEFAULT_MODEL } from '../src/defaults.mjs';

const tools = [{ type: 'function', function: { name: 'read_file', description: 'Read a file', parameters: { type: 'object', properties: { path: { type: 'string' } }, required: ['path'] } } }];
const user = [{ role: 'user', content: 'Read the file.' }];
class FakeRpc extends EventEmitter {
  ready = true; calls = []; replies = []; nextThread = 0; nextTurn = 0;
  async request(method, params) {
    this.calls.push({ method, params });
    if (method === 'model/list') return { data: [{ model: 'other-default', isDefault: true }, { model: DEFAULT_MODEL, isDefault: false, supportedReasoningEfforts: [{ reasoningEffort: 'medium' }, { reasoningEffort: 'high' }] }], nextCursor: null };
    if (method === 'thread/start') return { thread: { id: `thread-${++this.nextThread}` } };
    if (method === 'turn/start') {
      const id = `turn-${++this.nextTurn}`;
      queueMicrotask(() => this.onTurn?.(params.threadId, id));
      return { turn: { id } };
    }
    return {};
  }
  reply(id, result) { this.replies.push({ id, result }); this.onReply?.(id, result); }
  reject(id, message) { this.replies.push({ id, error: message }); }
  text(threadId, text) { this.emit('notification', { method: 'item/agentMessage/delta', params: { threadId, delta: text } }); }
  done(threadId, status = 'completed') { this.emit('notification', { method: 'turn/completed', params: { threadId, turn: { status } } }); }
  tool(threadId, id = 50) { this.emit('serverRequest', { id, method: 'item/tool/call', params: { threadId, turnId: 'turn-1', callId: `native-${id}`, tool: 'wb_tool_0', arguments: { path: 'sample.txt' } } }); }
}
function setup(t, options) {
  const rpc = new FakeRpc(); const bridge = new Bridge(rpc, { cwd: 'C:/test', ...options });
  t.after(() => bridge.close()); return { rpc, bridge };
}

test('text, delta streaming and subsequent user turn preserve native conversation', async t => {
  const { rpc, bridge } = setup(t);
  rpc.onTurn = id => { rpc.text(id, 'hello'); rpc.done(id); };
  const deltas = [];
  const result = await bridge.complete({ messages: user }, { onDelta: d => deltas.push(d) });
  assert.deepEqual(deltas, ['hello']);
  assert.equal(result.choices[0].finish_reason, 'stop');
  await bridge.complete({ messages: [...user, result.choices[0].message, { role: 'user', content: 'Next question' }] });
  assert.equal(rpc.nextThread, 1);
  assert.equal(rpc.calls.filter(c => c.method === 'turn/start')[1].params.input[0].text, 'Next question');
  const start = rpc.calls.find(c => c.method === 'thread/start').params;
  assert.deepEqual(start.environments, []);
  assert.equal(start.sandbox, 'read-only');
  assert.equal(start.model, 'gpt-6.1-sol');
  assert.equal(rpc.calls.find(c => c.method === 'turn/start').params.effort, 'high');
});

test('codex-local alias uses the fixed model; explicit effort remains supported', async t => {
  const { rpc, bridge } = setup(t);
  rpc.onTurn = id => { rpc.text(id, 'ok'); rpc.done(id); };
  const result = await bridge.complete({ model: 'codex-local', reasoning_effort: 'medium', messages: user });
  assert.equal(result.model, 'gpt-6.1-sol');
  assert.equal(rpc.calls.find(c => c.method === 'turn/start').params.effort, 'medium');
});

test('native dynamic tool call is returned to caller, result resolves original RPC', async t => {
  const { rpc, bridge } = setup(t);
  rpc.onTurn = id => rpc.tool(id);
  const first = await bridge.complete({ messages: user, tools });
  const assistant = first.choices[0].message;
  const call = assistant.tool_calls[0];
  assert.equal(first.choices[0].finish_reason, 'tool_calls');
  assert.equal(call.function.name, 'read_file');
  assert.equal(call.function.arguments, '{"path":"sample.txt"}');
  assert.equal(rpc.replies.length, 0);
  rpc.onReply = () => { rpc.text('thread-1', 'Value from WorkBuddy'); rpc.done('thread-1'); };
  const result = await bridge.complete({ tools, messages: [...user, { ...assistant, content: '' }, { role: 'tool', tool_call_id: call.id, content: 'file contents' }] });
  assert.equal(result.choices[0].message.content, 'Value from WorkBuddy');
  assert.deepEqual(rpc.replies[0], { id: 50, result: { success: true, contentItems: [{ type: 'inputText', text: 'file contents' }] } });
  assert.equal(rpc.nextTurn, 1);
});

test('parallel tool calls validate all outputs before resolving any request', async t => {
  const { rpc, bridge } = setup(t);
  rpc.onTurn = id => { rpc.tool(id, 50); rpc.tool(id, 51); };
  const first = await bridge.complete({ messages: user, tools });
  const assistant = first.choices[0].message;
  assert.equal(assistant.tool_calls.length, 2);
  await assert.rejects(bridge.complete({ tools, messages: [...user, assistant, { role: 'tool', tool_call_id: assistant.tool_calls[0].id, content: 'one' }] }), /完整回传/);
  assert.equal(rpc.replies.length, 0);
  rpc.onReply = () => { if (rpc.replies.length === 2) { rpc.text('thread-1', 'both'); rpc.done('thread-1'); } };
  const result = await bridge.complete({ tools, messages: [...user, assistant, ...assistant.tool_calls.map(c => ({ role: 'tool', tool_call_id: c.id, content: 'ok' }))] });
  assert.equal(result.choices[0].message.content, 'both');
});

test('tool calls arriving after one HTTP response are delivered on the next response', async t => {
  const { rpc, bridge } = setup(t);
  rpc.onTurn = id => rpc.tool(id, 50);
  const first = await bridge.complete({ messages: user, tools });
  rpc.tool('thread-1', 51);
  const history = [...user, first.choices[0].message, { role: 'tool', tool_call_id: first.choices[0].message.tool_calls[0].id, content: 'one' }];
  const second = await bridge.complete({ messages: history, tools });
  assert.equal(second.choices[0].finish_reason, 'tool_calls');
  assert.equal(second.choices[0].message.tool_calls.length, 1);
  assert.equal(rpc.replies.length, 1);
});

test('client cancellation interrupts Codex and releases session', async t => {
  const { rpc, bridge } = setup(t);
  const abort = new AbortController();
  rpc.onTurn = () => setImmediate(() => abort.abort());
  await assert.rejects(bridge.complete({ messages: user }, { signal: abort.signal }), /取消/);
  assert.equal(bridge.sessions.size, 0);
  assert.ok(rpc.calls.some(c => c.method === 'turn/interrupt'));
});

test('failed turns return an error, not successful partial content', async t => {
  const { rpc, bridge } = setup(t);
  rpc.onTurn = id => { rpc.text(id, 'partial'); rpc.done(id, 'failed'); };
  await assert.rejects(bridge.complete({ messages: user }), /failed/);
  assert.equal(bridge.sessions.size, 0);
});

test('expired tool results fail explicitly without starting another inference', async t => {
  const { rpc, bridge } = setup(t);
  await assert.rejects(bridge.complete({ messages: [...user, { role: 'tool', tool_call_id: 'missing', content: 'x' }], tools }), /过期/);
  assert.equal(rpc.nextThread, 0);
});

test('tool_choice none removes dynamic tools; unsupported required is rejected', () => {
  assert.deepEqual(toolsForCodex(tools, 'none').dynamicTools, []);
  assert.throws(() => toolsForCodex(tools, 'required'), /tool_choice/);
});

test('unsupported multimodal content is rejected explicitly', () => {
  assert.throws(() => normalizeMessages([{ role: 'user', content: [{ type: 'audio', data: 'x' }] }]), /只支持/);
});
