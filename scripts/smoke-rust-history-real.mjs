// Exercises a real Codex turn while WorkBuddy's follow-up history is shortened.
// This uses real Codex quota and runs an isolated server on port 18329.
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { createInterface } from 'node:readline';
import assert from 'node:assert/strict';
import { resolve } from 'node:path';

const suffix = process.platform === 'win32' ? '.exe' : '';
const child = spawn(resolve(`src-tauri/target/debug/examples/server_smoke${suffix}`), [process.argv[2]], {windowsHide:true,stdio:['pipe','pipe','inherit']});
const lines = createInterface({input:child.stdout});
const send = body => fetch('http://127.0.0.1:18329/v1/chat/completions', {method:'POST',headers:{authorization:'Bearer smoke-test-token-0123456789abcdef','content-type':'application/json'},body:JSON.stringify(body),signal:AbortSignal.timeout(120000)});
async function jsonRequest(body) {
  const response=await send(body); const value=await response.json();
  if (!response.ok) throw Error(value.error?.message || 'request failed');
  return value.choices[0].message;
}
try {
  assert.equal((await once(lines,'line'))[0],'READY');
  const tools=['get_first','get_second'].map(name=>({type:'function',function:{name,description:`Call ${name} to get its value`,parameters:{type:'object',properties:{},additionalProperties:false}}}));
  const messages=[{role:'user',content:'Call get_first once and report the returned value.'}];
  const first=await jsonRequest({messages,tools});
  const firstCall=first.tool_calls?.[0]; assert(firstCall && firstCall.function.name==='get_first','first tool not called');
  const base=[...messages,first,{role:'tool',tool_call_id:firstCall.id,content:'FIRST_VALUE_6351'}];
  const next=await jsonRequest({messages:[...base,{role:'user',content:'Also call get_second once, then report both tool values.'}],tools});
  const nextCall=next.tool_calls?.[0]; assert(nextCall && nextCall.function.name==='get_second','second tool not called');
  // WorkBuddy can omit the steered user message from the next history snapshot.
  const final=await jsonRequest({messages:[...base,next,{role:'tool',tool_call_id:nextCall.id,content:'SECOND_VALUE_6351'}],tools});
  assert(final.content?.includes('SECOND_VALUE_6351'),'second tool result missing');
  console.log('Real Codex accepted a second tool result after client history omitted the steered user message.');
} finally {
  child.stdin.end('stop\n');
  await once(child,'exit');
}
