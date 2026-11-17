// Uses real Codex quota; server_smoke must be built and receives the Codex executable path.
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { createInterface } from 'node:readline';
import assert from 'node:assert/strict';
import { resolve } from 'node:path';

const suffix = process.platform === 'win32' ? '.exe' : '';
const child = spawn(resolve(`src-tauri/target/debug/examples/server_smoke${suffix}`), [process.argv[2]], {windowsHide:true,stdio:['pipe','pipe','inherit']});
const lines = createInterface({input:child.stdout});
const send = body => fetch('http://127.0.0.1:18329/v1/chat/completions', {method:'POST',headers:{authorization:'Bearer smoke-test-token-0123456789abcdef','content-type':'application/json'},body:JSON.stringify(body),signal:AbortSignal.timeout(120000)});
try {
  assert.equal((await once(lines,'line'))[0],'READY');
  const tools=[{type:'function',function:{name:'get_test_value',description:'Get a test value',parameters:{type:'object',properties:{},additionalProperties:false}}}];
  const messages=[{role:'user',content:'Call get_test_value once, then report the exact value returned.'}];
  const firstResponse=await send({messages,tools});
  const first=await firstResponse.json();
  if (!firstResponse.ok) throw Error(first.error?.message || 'request failed');
  const call=first.choices[0].message.tool_calls?.[0];
  assert(call,'Codex did not call the test tool');
  const next=[...messages,first.choices[0].message,{role:'tool',tool_call_id:call.id,content:'REAL_TOOL_9274'},{role:'user',content:'Also include the exact text REAL_STEER_9274 in your answer.'}];
  const response=await send({messages:next,tools,stream:true});
  const stream=await response.text();
  const chunks=stream.split('\n').filter(line=>line.startsWith('data: {')).map(line=>JSON.parse(line.slice(6)));
  const error=chunks.find(chunk=>chunk.error)?.error;
  if (error) throw Error(error.message);
  const content=chunks.map(chunk=>chunk.choices?.[0]?.delta?.content || '').join('');
  assert(content.includes('REAL_TOOL_9274'),'tool result missing');
  assert(content.includes('REAL_STEER_9274'),'user instruction missing');
  console.log('Real HTTP bridge accepted tool result and new user instruction in the same turn.');
} finally {
  child.stdin.end('stop\n');
  await once(child,'exit');
}
