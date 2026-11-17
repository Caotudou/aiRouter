export class RouterError extends Error {
  constructor(message, status = 502, code = 'codex_error') {
    super(message);
    this.status = status;
    this.code = code;
  }
}

export function safeError(error) {
  return String(error?.message || error).replace(/Bearer\s+\S+/gi, 'Bearer [redacted]')
    .replace(/\b(?:sk-[\w-]+|eyJ[\w-]+\.[\w-]+\.[\w-]+)\b/g, '[redacted]').slice(0, 1200);
}
