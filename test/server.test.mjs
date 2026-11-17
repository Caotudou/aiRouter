import { test } from 'node:test';
import assert from 'node:assert/strict';
import { createApiServer } from '../src/server.mjs';

const token = 'a'.repeat(64);
async function setup(t, custom = {}) {
  const bridge = { rpc: { ready: true }, models: async () => [{ model: 'test-model' }],
    complete: async (body, { onStart, onDelta }) => {
      onStart({ id: 'chat-1', model: 'test-model' }); onDelta('hello');
      return { id: 'chat-1', model: 'test-model', choices: [{ index: 0, message: { role: 'assistant', content: 'hello' }, finish_reason: 'stop' }] };
    }, ...custom };
  const server = createApiServer({ bridge, token });
  await new Promise(r => server.listen(0, '127.0.0.1', r));
  t.after(async () => { server.closeAllConnections(); await new Promise(r => server.close(r)); });
  const url = `http://127.0.0.1:${server.address().port}`;
  const headers = { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' };
  return { server, url, headers };
}

test('API requires bearer key and rejects browser origins', async t => {
  const { url, headers } = await setup(t);
  assert.equal((await fetch(url + '/v1/models')).status, 401);
  assert.equal((await fetch(url + '/v1/models', { headers: { ...headers, Origin: 'https://example.com' } })).status, 403);
  const response = await fetch(url + '/v1/models', { headers });
  assert.equal(response.status, 200);
  assert.equal((await response.json()).data[0].id, 'codex-local');
});

test('nonstream completion and invalid JSON errors use JSON responses', async t => {
  const { url, headers } = await setup(t);
  const invalid = await fetch(url + '/v1/chat/completions', { method: 'POST', headers, body: '{' });
  assert.equal(invalid.status, 400);
  const response = await fetch(url + '/v1/chat/completions', { method: 'POST', headers, body: JSON.stringify({ messages: [{ role: 'user', content: 'Hi' }] }) });
  assert.equal((await response.json()).choices[0].message.content, 'hello');
});

test('SSE preserves IDs, ends with finish_reason and DONE', async t => {
  const { url, headers } = await setup(t);
  const response = await fetch(url + '/v1/chat/completions', { method: 'POST', headers, body: JSON.stringify({ stream: true, messages: [{ role: 'user', content: 'Hi' }] }) });
  assert.match(response.headers.get('content-type'), /text\/event-stream/);
  const text = await response.text();
  const chunks = text.split('\n\n').filter(s => s.startsWith('data: {')).map(s => JSON.parse(s.slice(6)));
  assert.ok(chunks.every(c => c.id === 'chat-1' && c.model === 'test-model'));
  assert.deepEqual(chunks[0].choices[0].delta, { role: 'assistant' });
  assert.equal(chunks.at(-1).choices[0].finish_reason, 'stop');
  assert.ok(text.endsWith('data: [DONE]\n\n'));
});

test('server errors redact authorization secrets', async t => {
  const { url, headers } = await setup(t, { complete: async () => { throw new Error('Bearer sk-private-token'); } });
  const response = await fetch(url + '/v1/chat/completions', { method: 'POST', headers, body: '{}' });
  const text = await response.text(); assert.equal(response.status, 500); assert.ok(!text.includes('sk-private-token'));
});
