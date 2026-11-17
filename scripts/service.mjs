import { randomBytes } from 'node:crypto';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { Controller } from '../src/controller.mjs';

const dataDir = process.env.AIROUTER_DATA_DIR || path.join(os.homedir(), '.ai-router');
await mkdir(dataDir, { recursive: true });
const configFile = path.join(dataDir, 'headless.json');
let config;
try { config = JSON.parse(await readFile(configFile, 'utf8')); }
catch (e) {
  if (e.code !== 'ENOENT') throw e;
  config = { token: randomBytes(32).toString('hex'), port: 18321, binary: 'codex' };
  await writeFile(configFile, JSON.stringify(config, null, 2), { mode: 0o600 });
}
const controller = new Controller({ dataDir, ...config, binary: process.env.CODEX_BIN || config.binary });
try {
  const state = await controller.start();
  console.log(`AI Router listening: ${state.baseUrl}`);
  console.log(`Local key is stored in: ${configFile}`);
} catch (e) { console.error(e.message); await controller.stop(); process.exitCode = 1; }
for (const signal of ['SIGINT', 'SIGTERM']) process.once(signal, async () => { await controller.stop(); process.exit(0); });
