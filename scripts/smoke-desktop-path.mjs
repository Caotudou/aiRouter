import path from 'node:path';
import { randomBytes } from 'node:crypto';
import { Controller } from '../src/controller.mjs';

// Reproduce an Explorer launch: no developer terminal Codex path or override.
for (const name of Object.keys(process.env)) {
  if (name.toLowerCase() === 'path' || name === 'CODEX_BIN') delete process.env[name];
}
process.env.Path = path.win32.join(process.env.SystemRoot || 'C:\\Windows', 'System32');
const controller = new Controller({ dataDir: path.resolve('.local/desktop-path-smoke'),
  token: randomBytes(32).toString('hex'), port: 18329, binary: 'codex' });
try {
  const status = await controller.start();
  console.log('Resolved executable:', status.resolvedBinary);
  console.log('Authentication:', status.account?.type);
  console.log('Inference:', await controller.selfTest());
  console.log('DESKTOP PATH SMOKE PASSED');
} finally { await controller.stop(); }
