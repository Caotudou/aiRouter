import { mkdir } from 'node:fs/promises';
import path from 'node:path';
import { randomBytes } from 'node:crypto';
import { Controller } from '../src/controller.mjs';

const dataDir = path.resolve('.local/smoke');
await mkdir(dataDir, { recursive: true });
const c = new Controller({ dataDir, token: randomBytes(32).toString('hex'), port: 18329, binary: process.env.CODEX_BIN || 'codex' });
async function send(body) {
  const res = await fetch(`http://127.0.0.1:${c.port}/v1/chat/completions`, {
    method: 'POST', headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${c.token}` },
    body: JSON.stringify({ model: 'codex-local', ...body }), signal: AbortSignal.timeout(120000),
  });
  const data = body.stream ? await res.text() : await res.json();
  if (!res.ok) throw new Error(JSON.stringify(data));
  return data;
}
try {
  const state = await c.start();
  console.log('Auth:', state.account.type, '| Models:', state.models.length);
  const text = await send({ messages: [{ role: 'user', content: 'Reply with exactly: AI Router OK' }] });
  console.log('Text:', text.choices[0].message.content);
  if (!text.choices[0].message.content?.includes('AI Router OK')) throw new Error('Text response mismatch.');
  const stream = await send({ stream: true, messages: [{ role: 'user', content: 'Reply with exactly: Stream OK' }] });
  if (!stream.includes('data: [DONE]') || !stream.includes('chat.completion.chunk')) throw new Error('SSE response invalid.');
  console.log('SSE: passed');
  const tools = [{ type: 'function', function: { name: 'get_bridge_test_value', description: 'Return the test value. You must call this to learn the value.', parameters: { type: 'object', properties: {}, additionalProperties: false } } }];
  const messages = [{ role: 'user', content: 'Call get_bridge_test_value once and report the exact value returned. Do not guess it.' }];
  const called = await send({ tools, messages });
  const call = called.choices[0].message.tool_calls?.[0];
  if (!call || call.function.name !== 'get_bridge_test_value') throw new Error('No matching tool call: ' + JSON.stringify(called));
  console.log('Tool call:', call.function.name);
  const completed = await send({ tools, messages: [...messages, called.choices[0].message, { role: 'tool', tool_call_id: call.id, content: 'BRIDGE_REAL_7392' }] });
  if (!completed.choices[0].message.content?.includes('BRIDGE_REAL_7392')) throw new Error('Tool result not used: ' + JSON.stringify(completed));
  console.log('Tool round trip:', completed.choices[0].message.content);
  console.log('REAL SMOKE PASSED');
} finally { await c.stop(); }
