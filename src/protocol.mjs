import { RouterError } from './errors.mjs';

export const canonical = (value) => JSON.stringify(value, (_, v) => {
  if (v && typeof v === 'object' && !Array.isArray(v)) {
    return Object.fromEntries(Object.keys(v).sort().map(k => [k, v[k]]));
  }
  return v;
});

export function normalizeMessages(messages) {
  if (!Array.isArray(messages) || !messages.length) throw new RouterError('messages 必须是非空数组。', 400, 'invalid_request');
  return messages.map((m) => {
    if (!m || !['system', 'developer', 'user', 'assistant', 'tool'].includes(m.role)) {
      throw new RouterError('不支持的消息角色。', 400, 'invalid_request');
    }
    const out = { role: m.role, content: m.content ?? null };
    if (typeof out.content !== 'string' && out.content !== null && !Array.isArray(out.content)) {
      throw new RouterError('content 必须为文本或内容数组。', 400, 'invalid_request');
    }
    if (Array.isArray(out.content)) {
      for (const p of out.content) {
        if (p.type === 'text' && typeof p.text === 'string') continue;
        if (p.type === 'image_url' && typeof p.image_url?.url === 'string') continue;
        throw new RouterError('首版只支持文本和 image_url 图片内容。', 400, 'unsupported_content');
      }
    }
    if (m.name !== undefined) out.name = m.name;
    if (m.tool_calls !== undefined) {
      if (!Array.isArray(m.tool_calls) || m.tool_calls.some(t => !t.id || t.type !== 'function' || typeof t.function?.name !== 'string' || typeof t.function?.arguments !== 'string')) {
        throw new RouterError('tool_calls 格式无效。', 400, 'invalid_request');
      }
      out.tool_calls = m.tool_calls.map(t => ({ id: t.id, type: 'function', function: { name: t.function.name, arguments: t.function.arguments } }));
      if (out.content === '') out.content = null;
    }
    if (m.role === 'tool') {
      if (typeof m.tool_call_id !== 'string') throw new RouterError('工具结果缺少 tool_call_id。', 400, 'invalid_request');
      out.tool_call_id = m.tool_call_id;
    }
    return out;
  });
}

export function toolsForCodex(tools = [], choice) {
  if (!Array.isArray(tools)) throw new RouterError('tools 必须为数组。', 400, 'invalid_request');
  if (choice && !['auto', 'none'].includes(choice)) {
    throw new RouterError('首版支持 tool_choice=auto/none，暂不支持强制指定工具。', 400, 'unsupported_parameter');
  }
  const names = new Map();
  const dynamicTools = tools.map((t, i) => {
    if (t?.type !== 'function' || !t.function?.name || !t.function.parameters || typeof t.function.parameters !== 'object') {
      throw new RouterError('首版仅支持具有 parameters 的 function 工具。', 400, 'invalid_request');
    }
    // Avoid collisions with Codex built-ins and Responses name restrictions.
    const name = `wb_tool_${i}`;
    names.set(name, t.function.name);
    return { type: 'function', name, description: `${t.function.name}: ${t.function.description || ''}`, inputSchema: t.function.parameters };
  });
  return { names, dynamicTools: choice === 'none' ? [] : dynamicTools };
}

export function textContent(content) {
  if (typeof content === 'string') return content;
  return (content || []).filter(p => p.type === 'text').map(p => p.text).join('\n');
}

export function userInput(messages) {
  const input = [];
  for (const m of messages) {
    if (m.role === 'system' || m.role === 'developer') continue;
    const text = textContent(m.content);
    if (text) input.push({ type: 'text', text: m.role === 'user' ? text : `[${m.role}] ${text}`, text_elements: [] });
    if (Array.isArray(m.content)) for (const p of m.content) {
      if (p.type === 'image_url') input.push({ type: 'image', url: p.image_url.url, detail: p.image_url.detail || 'auto' });
    }
    if (m.tool_calls) input.push({ type: 'text', text: `Previous assistant tool calls: ${JSON.stringify(m.tool_calls)}`, text_elements: [] });
  }
  return input;
}

export function toolOutput(content) {
  if (typeof content === 'string' || content === null) return [{ type: 'inputText', text: content || '' }];
  return content.map(p => p.type === 'text' ? { type: 'inputText', text: p.text } : { type: 'inputImage', imageUrl: p.image_url.url });
}

export function validateOptions(body) {
  if (body.stream !== undefined && typeof body.stream !== 'boolean') throw new RouterError('stream 必须为布尔值。', 400, 'invalid_request');
  if (body.n !== undefined && body.n !== 1) throw new RouterError('仅支持 n=1。', 400, 'unsupported_parameter');
  for (const name of ['response_format', 'logprobs', 'logit_bias', 'seed', 'stop']) {
    if (body[name] != null && body[name] !== false) throw new RouterError(`首版不支持 ${name}。`, 400, 'unsupported_parameter');
  }
}
