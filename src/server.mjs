import http from 'node:http';
import { timingSafeEqual } from 'node:crypto';
import { RouterError, safeError } from './errors.mjs';

function json(res, status, data) {
  res.writeHead(status, { 'Content-Type': 'application/json; charset=utf-8', 'Cache-Control': 'no-store' });
  res.end(JSON.stringify(data));
}

async function readBody(req, limit = 16 * 1024 * 1024) {
  let size = 0; const chunks = [];
  for await (const chunk of req) {
    size += chunk.length;
    if (size > limit) throw new RouterError('请求内容超过 16 MB。', 413, 'request_too_large');
    chunks.push(chunk);
  }
  try { return JSON.parse(Buffer.concat(chunks).toString('utf8')); }
  catch { throw new RouterError('请求必须是有效 JSON。', 400, 'invalid_json'); }
}

export function createApiServer({ bridge, token, log = () => {} }) {
  if (!token || token.length < 24) throw new Error('Local API key must contain at least 24 characters.');
  const server = http.createServer(async (req, res) => {
    const controller = new AbortController();
    res.once('close', () => { if (!res.writableEnded) controller.abort(); });
    let heartbeat;
    try {
      const host = (req.headers.host || '').split(':')[0];
      if (!['127.0.0.1', 'localhost'].includes(host)) throw new RouterError('仅允许本机访问。', 403, 'invalid_host');
      if (req.headers.origin) throw new RouterError('不接受网页跨域调用。', 403, 'invalid_origin');
      const supplied = Buffer.from(req.headers.authorization || '');
      const expected = Buffer.from(`Bearer ${token}`);
      if (supplied.length !== expected.length || !timingSafeEqual(supplied, expected)) throw new RouterError('本机 API Key 无效。', 401, 'invalid_api_key');
      const path = new URL(req.url, 'http://localhost').pathname;
      if (req.method === 'GET' && path === '/health') return json(res, 200, { status: 'ok', codexConnected: bridge.rpc.ready });
      if (req.method === 'GET' && ['/v1/models', '/models'].includes(path)) {
        const models = await bridge.models();
        return json(res, 200, { object: 'list', data: [{ id: 'codex-local', object: 'model', owned_by: 'local' },
          ...models.map(m => ({ id: m.model, object: 'model', owned_by: 'openai' }))] });
      }
      if (req.method !== 'POST' || !['/v1/chat/completions', '/chat/completions'].includes(path)) {
        throw new RouterError('未找到接口。支持 /v1/models 和 /v1/chat/completions。', 404, 'not_found');
      }
      if (!req.headers['content-type']?.includes('application/json')) throw new RouterError('请使用 application/json。', 415, 'invalid_content_type');
      const body = await readBody(req);
      if (!body || typeof body !== 'object' || Array.isArray(body)) throw new RouterError('请求必须为 JSON 对象。', 400, 'invalid_request');
      let streamId; let streamModel; const created = Math.floor(Date.now() / 1000);
      const chunk = (delta, finish_reason = null) => {
        if (!res.headersSent) {
          res.writeHead(200, { 'Content-Type': 'text/event-stream; charset=utf-8', 'Cache-Control': 'no-cache', 'Connection': 'keep-alive', 'X-Accel-Buffering': 'no' });
        }
        res.write(`data: ${JSON.stringify({ id: streamId, object: 'chat.completion.chunk', created, model: streamModel,
          choices: [{ index: 0, delta, finish_reason }] })}\n\n`);
      };
      const result = await bridge.complete(body, { signal: controller.signal, onStart: meta => {
        if (!body.stream || res.destroyed) return;
        streamId = meta.id; streamModel = meta.model;
        chunk({ role: 'assistant' });
        heartbeat = setInterval(() => { if (!res.destroyed) res.write(': keep-alive\n\n'); }, 15000);
      }, onDelta: delta => {
        if (!body.stream || res.destroyed) return;
        streamId ||= `chatcmpl-stream-${Date.now()}`; streamModel ||= body.model || 'codex-local';
        if (!res.headersSent) chunk({ role: 'assistant' });
        chunk({ content: delta });
      } });
      if (!body.stream) return json(res, 200, result);
      streamId ||= result.id; streamModel ||= result.model;
      if (!res.headersSent) chunk({ role: 'assistant' });
      const choice = result.choices[0];
      if (choice.message.tool_calls) {
        chunk({ tool_calls: choice.message.tool_calls.map((c, index) => ({ index, ...c })) });
      }
      chunk({}, choice.finish_reason);
      if (body.stream_options?.include_usage) res.write(`data: ${JSON.stringify({ id: streamId, object: 'chat.completion.chunk',
        created, model: streamModel, choices: [], usage: result.usage || null })}\n\n`);
      res.end('data: [DONE]\n\n');
    } catch (error) {
      if (res.destroyed) return;
      const message = safeError(error);
      const data = { error: { message, type: 'router_error', code: error.code || 'internal_error' } };
      log({ type: 'error', message });
      if (res.headersSent) res.end(`data: ${JSON.stringify(data)}\n\ndata: [DONE]\n\n`);
      else json(res, error.status || 500, data);
    } finally { clearInterval(heartbeat); }
  });
  server.requestTimeout = 60000;
  server.headersTimeout = 15000;
  return server;
}
