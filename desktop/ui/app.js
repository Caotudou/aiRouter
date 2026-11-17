const $ = id => document.getElementById(id);
let state, working = false, draft = null;
function show(message, error = false) { $('message').hidden = false; $('message').textContent = message; $('message').classList.toggle('error', error); }
const effortLabels = { minimal:'最低', low:'低', medium:'中', high:'高', xhigh:'极高', max:'最高', ultra:'超高' };
function replaceOptions(select, entries, selected) {
  const signature = JSON.stringify(entries);
  if (select.dataset.signature !== signature) {
    select.replaceChildren(...entries.map(([value,label]) => { const option = document.createElement('option'); option.value=value; option.textContent=label; return option; }));
    select.dataset.signature = signature;
  }
  if (select.value !== selected) select.value = selected;
}
function renderDefaults(s) {
  const available = typeof window.router.setDefaults === 'function';
  $('model-settings').hidden = !available;
  if (!available) return;
  const models = Array.isArray(s.models) ? s.models.filter(m => typeof m.model === 'string' && m.model) : [];
  const model = draft?.model || s.defaultModel;
  const entries = models.map(m => [m.model, m.displayName && m.displayName !== m.model ? `${m.displayName} (${m.model})` : m.model]);
  if (!entries.some(([value]) => value === model)) entries.unshift([model, model]);
  replaceOptions($('default-model'), entries, model);
  const descriptor = models.find(m => m.model === model);
  const efforts = (descriptor?.supportedReasoningEfforts || []).map(e => typeof e === 'string' ? e : e?.reasoningEffort).filter(Boolean);
  let effort = draft?.effort || s.defaultReasoningEffort;
  if (efforts.length && !efforts.includes(effort)) {
    effort = efforts.includes(descriptor?.defaultReasoningEffort) ? descriptor.defaultReasoningEffort : efforts[0];
    if (draft) draft.effort = effort;
  }
  if (!efforts.length) efforts.push(effort);
  replaceOptions($('default-effort'), efforts.map(value => [value, `${effortLabels[value] || value}（${value}）`]), effort);
  $('default-model').disabled = working;
  $('default-effort').disabled = working;
  $('apply-defaults').disabled = working || !draft || (model === s.defaultModel && effort === s.defaultReasoningEffort);
}
function render(s) {
  state = s;
  renderDefaults(s);
  $('service-state').textContent = s.running ? '服务运行中' : '服务未启动';
  $('service-dot').classList.toggle('running', s.running);
  $('service-description').textContent = s.running ? `正在监听本机端口 ${s.port}，可接收 WorkBuddy 请求。` : '启动后，将下方接入信息填写到 WorkBuddy。';
  $('toggle').textContent = s.running ? '停止服务 □' : '启动服务 ↗';
  $('toggle').classList.toggle('stop', s.running);
  $('test').disabled = !s.running || working;
  $('base-url').textContent = s.baseUrl;
  $('model-name').textContent = typeof window.router.setDefaults === 'function' ? 'codex-local' : s.defaultModel;
  $('connect-note').textContent = typeof window.router.setDefaults === 'function' ? `codex-local 当前对应 ${s.defaultModel}，默认强度 ${s.defaultReasoningEffort}。启用工具调用。` : '选择 OpenAI 兼容协议，启用工具调用。';
  $('account-tag').textContent = s.account?.type === 'chatgpt' ? '已登录' : '未登录';
  $('account-plan').textContent = s.account?.planType ? `ChatGPT ${s.account.planType[0].toUpperCase() + s.account.planType.slice(1)}` : '未连接';
  $('account-description').textContent = s.account?.type === 'chatgpt' ? `${s.models.length} 个目录模型 · 默认 ${s.defaultModel} · 推理强度 ${effortLabels[s.defaultReasoningEffort] || s.defaultReasoningEffort}（${s.defaultReasoningEffort}）` : '请先打开 Codex，使用 ChatGPT 账户登录。';
  if (document.activeElement !== $('port')) $('port').value = s.port;
  if (document.activeElement !== $('binary')) $('binary').value = s.binary;
  $('resolved-binary').textContent = s.resolvedBinary ? `实际程序：${s.resolvedBinary}` : '填写 codex 可自动查找本机安装；也可以选择实际的 codex.exe。';
  $('save').disabled = s.running || working; $('browse').disabled = s.running || working;
  $('port').disabled = s.running; $('binary').disabled = s.running;
  $('quota').replaceChildren();
  const limits = s.quota?.rateLimits;
  for (const key of ['primary', 'secondary']) {
    const bucket = limits?.[key]; if (!bucket) continue;
    const row = document.createElement('div');
    const label = document.createElement('div'); label.className = 'quota-label';
    const title = document.createElement('span');
    const mins = bucket.windowDurationMins;
    title.textContent = mins === 10080 ? '每周用量' : mins % 60 === 0 ? `${mins / 60} 小时用量` : `${mins} 分钟用量`;
    const value = document.createElement('strong'); value.textContent = `已用 ${bucket.usedPercent}%`;
    label.append(title, value);
    const progress = document.createElement('progress'); progress.max = 100; progress.value = bucket.usedPercent;
    progress.setAttribute('aria-label', `${title.textContent}：已用 ${bucket.usedPercent}%`);
    const reset = document.createElement('div'); reset.className = 'reset-time';
    reset.textContent = bucket.resetsAt ? `${new Date(bucket.resetsAt * 1000).toLocaleString('zh-CN', { month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit' })} 重置` : '重置时间未知';
    row.append(label, progress, reset); $('quota').append(row);
  }
  if (!$('quota').children.length) { const p = document.createElement('p'); p.textContent = s.quota?.error || '暂无额度数据'; $('quota').append(p); }
  $('sessions').textContent = `${s.activeSessions} 个会话`;
  $('logs').replaceChildren();
  for (const event of s.logs) {
    const row = document.createElement('div'); row.className = `log-row ${event.type === 'error' ? 'log-error' : ''}`;
    const status = { started: '服务已启动', stopped: '服务已停止', completed: '请求已完成', tool_calls: '工具调用已交给 WorkBuddy' }[event.status] || event.message;
    row.textContent = `${new Date(event.at).toLocaleTimeString('zh-CN')}  ${status}${event.model ? ` · ${event.model}` : ''}`;
    $('logs').append(row);
  }
  if (!s.logs.length) $('logs').textContent = '暂无运行记录';
  if (s.error) show(s.error, true);
}
async function run(button, fn) {
  if (working) return;
  working = true; const original = button.textContent; button.disabled = true; button.textContent = '处理中…';
  try { await fn(); } catch (e) { show(String(e?.message || e).replace(/^Error invoking remote method '[^']+': Error: /, ''), true); }
  finally { working = false; button.disabled = false; button.textContent = original; render(await window.router.status()); }
}
$('refresh').onclick = () => run($('refresh'), async () => render(await window.router.refresh()));
$('toggle').onclick = () => run($('toggle'), async () => {
  render(await (state?.running ? window.router.stop() : window.router.start()));
  show(state.running ? '服务已启动，可以在 WorkBuddy 中填写接入信息。' : '服务已停止。');
});
$('test').onclick = () => run($('test'), async () => { const reply = await window.router.test(); $('test-result').textContent = `Codex 响应：${reply}`; show('连接测试成功，本机服务已收到 Codex 的真实响应。'); });
$('save').onclick = () => run($('save'), async () => { render(await window.router.save({ port: Number($('port').value), binary: $('binary').value })); show('设置已保存。'); });
$('default-model').onchange = () => { draft = { model:$('default-model').value, effort:$('default-effort').value }; renderDefaults(state); };
$('default-effort').onchange = () => { draft = { model:$('default-model').value, effort:$('default-effort').value }; renderDefaults(state); };
$('apply-defaults').onclick = () => run($('apply-defaults'), async () => {
  const updated = await window.router.setDefaults($('default-model').value, $('default-effort').value);
  draft = null; render(updated); show('模型和推理强度已应用，新请求立即生效。');
});
$('browse').onclick = async () => { const binary = await window.router.chooseBinary(); if (binary) $('binary').value = binary; };
for (const button of document.querySelectorAll('[data-copy]')) button.onclick = () => run(button, async () => { await window.router.copy(button.dataset.copy); show('已复制到剪贴板。'); });
$('copy-all').onclick = () => run($('copy-all'), async () => { await window.router.copy('all'); show('已复制 Base URL、API Key 和模型名称。'); });
window.router.refresh().then(render).catch(e => show(String(e?.message || e), true));
setInterval(async () => { if (!working) render(await window.router.status()); }, 3000);
