(() => {
  if (typeof window.router?.usage !== 'function') return;

  const get = id => document.getElementById(id);
  const panel = get('usage');
  const range = get('usage-range');
  const start = get('usage-start');
  const end = get('usage-end');
  const hourMs = 3_600_000;
  const count = new Intl.NumberFormat('zh-CN');
  const compact = new Intl.NumberFormat('zh-CN', { notation: 'compact', maximumFractionDigits: 1 });
  const time = new Intl.DateTimeFormat('zh-CN', { month: 'numeric', day: 'numeric', hour: '2-digit', minute: '2-digit', hourCycle: 'h23' });
  let report;
  let queryId = 0;

  function localHour(timestamp) {
    const date = new Date(timestamp);
    return new Date(timestamp - date.getTimezoneOffset() * 60_000).toISOString().slice(0, 16);
  }

  function currentHourEnd() {
    const now = new Date();
    now.setMinutes(0, 0, 0);
    return now.getTime() + hourMs;
  }

  async function refresh() {
    const id = ++queryId;
    const status = get('usage-status');
    const results = get('usage-results');
    const custom = range.value === 'custom';
    const until = custom ? new Date(end.value).getTime() : currentHourEnd();
    const since = custom ? new Date(start.value).getTime() : until - Number(range.value) * hourMs;
    if ((custom && (!start.validity.valid || !end.validity.valid)) || !Number.isFinite(since) || !Number.isFinite(until) || until <= since || until - since > 168 * hourMs || until - since < hourMs || since < 0) {
      status.textContent = '请选择开始和结束时间，范围为 1 小时至 7 天。';
      results.hidden = true;
      return;
    }
    if (!report) status.textContent = '正在读取本机用量…';
    try {
      const response = await window.router.usage(since, until);
      if (id !== queryId) return;
      report = response;
      render();
    } catch (error) {
      if (id !== queryId) return;
      results.hidden = true;
      status.textContent = String(error?.message || error);
    }
  }

  function render() {
    const { hours, incomplete } = report;
    const totals = hours.reduce((sum, hour) => ({
      requests: sum.requests + hour.requests,
      total: sum.total + hour.tokens.total,
      input: sum.input + hour.tokens.input,
      output: sum.output + hour.tokens.output,
      cached: sum.cached + hour.tokens.cachedInput,
    }), { requests: 0, total: 0, input: 0, output: 0, cached: 0 });
    const available = hours.some(hour => hour.shortQuota || hour.weeklyQuota);
    const hasTokens = totals.total > 0;
    const messages = [];
    if (!totals.requests && !hasTokens) messages.push('这段时间暂无转发用量。');
    if (totals.requests && !hasTokens) messages.push('这段时间有请求，但尚无 Codex Token 用量上报。');
    if (!available) messages.push('这段时间暂无账户额度采样。');
    if (incomplete) messages.push('部分记录未能写入或读取，统计可能不完整。');
    get('usage-status').textContent = messages.join(' ');
    get('usage-requests').textContent = count.format(totals.requests);
    get('usage-tokens').textContent = hasTokens ? count.format(totals.total) : '—';
    get('usage-in-out').textContent = hasTokens ? `${count.format(totals.input)} / ${count.format(totals.output)}` : '—';
    get('usage-cached').textContent = hasTokens ? count.format(totals.cached) : '—';
    const numberOrDash = (value, present) => present ? count.format(value) : '—';
    const quotaText = value => value ? `${count.format(value.usedPercent)}%` : '—';
    const rows = hours.filter(hour => hour.requests || hour.tokens.total || hour.shortQuota || hour.weeklyQuota).reverse();
    get('usage-rows').replaceChildren(...rows.map(hour => {
      const row = document.createElement('tr');
      const known = hour.tokens.total > 0;
      const values = [time.format(hour.start), count.format(hour.requests), numberOrDash(hour.tokens.input, known), numberOrDash(hour.tokens.output, known), numberOrDash(hour.tokens.cachedInput, known), numberOrDash(hour.tokens.total, known), quotaText(hour.shortQuota), quotaText(hour.weeklyQuota)];
      values.forEach((value, i) => {
        const cell = document.createElement(i === 0 ? 'th' : 'td');
        if (i === 0) cell.scope = 'row';
        cell.textContent = value;
        row.append(cell);
      });
      return row;
    }));
    get('usage-results').hidden = !rows.length;
    draw();
  }

  function plot(canvas, maximum, percent) {
    const { width, height } = canvas.getBoundingClientRect();
    if (!width || !height) return null;
    const ratio = window.devicePixelRatio || 1;
    canvas.width = Math.round(width * ratio);
    canvas.height = Math.round(height * ratio);
    const ctx = canvas.getContext('2d');
    if (!ctx) return null;
    ctx.scale(ratio, ratio);
    const left = 47;
    const right = width - 10;
    const top = 10;
    const bottom = height - 27;
    const hours = report.hours;
    const step = (right - left) / hours.length;
    const x = i => left + step * (i + 0.5);
    const y = value => bottom - (bottom - top) * Math.min(value, maximum) / maximum;
    ctx.font = '11px Consolas, monospace';
    ctx.textAlign = 'right';
    ctx.textBaseline = 'middle';
    ctx.fillStyle = '#5b6b68';
    ctx.strokeStyle = '#dce4e1';
    for (const value of [0, maximum / 2, maximum]) {
      const line = y(value);
      ctx.beginPath(); ctx.moveTo(left, line); ctx.lineTo(right, line); ctx.stroke();
      ctx.fillText(percent ? `${value}%` : compact.format(Math.round(value)), left - 7, line);
    }
    ctx.textAlign = 'left';
    ctx.fillText(time.format(hours[0].start), left, height - 12);
    ctx.textAlign = 'right';
    if (hours.length > 1) ctx.fillText(time.format(hours.at(-1).start), right, height - 12);
    return { ctx, x, y, step, bottom };
  }

  function draw() {
    if (!report || !panel.open || get('usage-results').hidden) return;
    const hours = report.hours;
    const max = Math.max(2, ...hours.map(hour => hour.tokens.total));
    const barChart = plot(get('usage-token-chart'), max, false);
    if (barChart) {
      const { ctx, x, y, step, bottom } = barChart;
      ctx.fillStyle = '#146b54';
      const width = Math.max(1, step * 0.75);
      hours.forEach((hour, i) => {
        if (!hour.tokens.total) return;
        const top = y(hour.tokens.total);
        ctx.fillRect(x(i) - width / 2, top, width, bottom - top);
      });
    }
    const lineChart = plot(get('usage-quota-chart'), 100, true);
    if (lineChart) {
      const { ctx, x, y } = lineChart;
      for (const [key, color] of [['shortQuota', '#346590'], ['weeklyQuota', '#146b54']]) {
        let previous;
        ctx.lineWidth = 1.6;
        ctx.strokeStyle = color;
        ctx.fillStyle = color;
        hours.forEach((hour, i) => {
          const current = hour[key];
          if (!current) { previous = undefined; return; }
          const pos = y(current.usedPercent);
          if (previous && previous.resetsAt === current.resetsAt && current.usedPercent >= previous.usedPercent) {
            ctx.beginPath(); ctx.moveTo(x(i - 1), y(previous.usedPercent)); ctx.lineTo(x(i), pos); ctx.stroke();
          }
          ctx.beginPath(); ctx.arc(x(i), pos, 2, 0, 2 * Math.PI); ctx.fill();
          previous = current;
        });
      }
    }
  }

  panel.hidden = false;
  get('usage-nav').hidden = false;
  get('usage-nav').onclick = () => { panel.open = true; };
  panel.addEventListener('toggle', () => { if (panel.open) refresh(); });
  range.addEventListener('change', () => {
    get('usage-custom').hidden = range.value !== 'custom';
    if (range.value === 'custom' && !start.value) {
      const until = currentHourEnd();
      start.value = localHour(until - 24 * hourMs);
      end.value = localHour(until);
    }
    refresh();
  });
  for (const input of [start, end]) input.addEventListener('change', refresh);
  new ResizeObserver(draw).observe(get('usage-results'));
  setInterval(() => { if (panel.open && !document.hidden) refresh(); }, 30_000);
})();
