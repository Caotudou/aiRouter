import path from 'node:path';
import { stat } from 'node:fs/promises';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { RouterError } from './errors.mjs';

const runFile = promisify(execFile);

async function isFile(file) {
  try { return (await stat(file)).isFile(); } catch { return false; }
}

async function appxLocations(env) {
  const powershell = path.win32.join(env.SystemRoot || 'C:\\Windows', 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe');
  const { stdout } = await runFile(powershell, [
    '-NoLogo', '-NoProfile', '-NonInteractive', '-Command',
    '@((Get-AppxPackage -Name OpenAI.Codex).InstallLocation) | ConvertTo-Json -Compress',
  ], { windowsHide: true, timeout: 10000, maxBuffer: 256 * 1024, env });
  const locations = JSON.parse(stdout.trim() || '[]');
  return (Array.isArray(locations) ? locations : [locations]).filter(v => typeof v === 'string' && v.length);
}

// Explorer and the developer terminal can have different PATH values. Resolve
// the native executable explicitly; never invoke a user-supplied shell command.
export async function resolveCodexBinary(configured = 'codex', {
  env = process.env, platform = process.platform, lookupAppx = appxLocations,
} = {}) {
  let requested = configured.trim();
  const automatic = /^(codex|codex\.exe)$/i.test(requested);
  if (automatic && env.CODEX_BIN) requested = env.CODEX_BIN.trim();
  if (!/^(codex|codex\.exe)$/i.test(requested)) {
    if (platform === 'win32' && !requested.toLowerCase().endsWith('.exe')) {
      throw new RouterError('请选择实际的 codex.exe，不能使用 .cmd、.ps1 或带参数的命令。', 503, 'codex_not_found');
    }
    if (await isFile(requested)) return path.resolve(requested);
    throw new RouterError(`Codex 程序不存在：${requested}。请重新选择 codex.exe，或填写 codex 使用自动检测。`, 503, 'codex_not_found');
  }
  if (platform !== 'win32') return requested;

  const pathValue = Object.entries(env).find(([key]) => key.toLowerCase() === 'path')?.[1] || '';
  const candidates = pathValue.split(';').filter(Boolean).map(dir => path.win32.join(dir.replace(/^"|"$/g, ''), 'codex.exe'));
  if (env.LOCALAPPDATA) candidates.push(path.win32.join(env.LOCALAPPDATA, 'OpenAI', 'Codex', 'bin', 'codex.exe'));
  if (env.USERPROFILE) candidates.push(path.win32.join(env.USERPROFILE, '.local', 'bin', 'codex.exe'));
  for (const candidate of candidates) if (await isFile(candidate)) return candidate;

  try {
    for (const location of await lookupAppx(env)) {
      const candidate = path.win32.join(location, 'app', 'resources', 'codex.exe');
      if (await isFile(candidate)) return candidate;
    }
  } catch { /* Store discovery may be unavailable; offer an explicit path. */ }
  throw new RouterError('没有找到 Codex。已检查 PATH 和 Microsoft Store 安装位置，请在连接设置中选择 codex.exe。', 503, 'codex_not_found');
}
