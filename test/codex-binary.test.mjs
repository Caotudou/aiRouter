import { test } from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, writeFile, rm } from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import { resolveCodexBinary } from '../src/codex-binary.mjs';

async function fixture(t) {
  const root = await mkdtemp(path.join(os.tmpdir(), 'ai-router-binary-'));
  t.after(() => {
    assert.equal(path.dirname(path.resolve(root)), path.resolve(os.tmpdir()));
    assert.ok(path.basename(root).startsWith('ai-router-binary-'));
    return rm(root, { recursive: true, force: true });
  });
  const binary = path.join(root, 'app', 'resources', 'codex.exe');
  await mkdir(path.dirname(binary), { recursive: true });
  await writeFile(binary, 'test fixture');
  return { root, binary };
}

test('finds Microsoft Store binary with an empty desktop PATH', async t => {
  const { root, binary } = await fixture(t);
  let searched = false;
  const actual = await resolveCodexBinary('codex', { env: { Path: '' }, platform: 'win32', lookupAppx: async () => { searched = true; return [root]; } });
  assert.equal(actual, binary); assert.equal(searched, true);
});

test('resolves an executable in case-insensitive Windows Path before Store discovery', async t => {
  const { binary } = await fixture(t);
  const actual = await resolveCodexBinary('codex', { env: { Path: `"${path.dirname(binary)}"` }, platform: 'win32', lookupAppx: async () => { throw new Error('Must not query Store'); } });
  assert.equal(actual, binary);
});

test('explicit paths and CODEX_BIN remain authoritative', async t => {
  const { binary } = await fixture(t);
  assert.equal(await resolveCodexBinary(binary, { platform: 'win32', env: {} }), binary);
  assert.equal(await resolveCodexBinary('codex', { platform: 'win32', env: { CODEX_BIN: binary } }), binary);
  await assert.rejects(resolveCodexBinary(path.join(path.dirname(binary), 'missing.exe'), { platform: 'win32', env: {} }), /程序不存在/);
});

test('missing installation returns an actionable error without starting a shell', async () => {
  await assert.rejects(resolveCodexBinary('codex', { platform: 'win32', env: {}, lookupAppx: async () => [] }), /没有找到 Codex/);
  await assert.rejects(resolveCodexBinary('codex.cmd', { platform: 'win32', env: {} }), /实际的 codex.exe/);
});
