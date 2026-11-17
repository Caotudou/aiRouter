import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { createInterface } from 'node:readline';
import assert from 'node:assert/strict';
import { resolve } from 'node:path';
import { mkdirSync, readFileSync, unlinkSync } from 'node:fs';

const suffix = process.platform === 'win32' ? '.exe' : '';
const dir = resolve('src-tauri/target/debug/examples');
mkdirSync(resolve('.local'), {recursive:true});
const diagnosticFile = resolve('.local', `diagnostics-test-${process.pid}.jsonl`);
const child = spawn(`${dir}/server_smoke${suffix}`, [`${dir}/fake_codex${suffix}`], { stdio: ['pipe', 'pipe', 'inherit'], windowsHide: true, env:{...process.env, AI_ROUTER_DIAGNOSTICS_PATH:diagnosticFile} });
const lines = createInterface({ input: child.stdout });
const url = 'http://127.0.0.1:18329';
const headers = { authorization: 'Bearer smoke-test-token-0123456789abcdef', 'content-type': 'application/json' };
const request = (body, extra = {}) => fetch(`${url}/v1/chat/completions`, { method:'POST', headers, body: JSON.stringify(body), signal: AbortSignal.timeout(5000), ...extra });
const msg = content => [{role:'user',content}];
try {
  assert.equal((await once(lines,'line'))[0], 'READY');
  assert.equal((await fetch(`${url}/v1/models`)).status, 401);
  assert.equal((await fetch(`${url}/v1/models`, {headers: {...headers, origin:'https://example.com'}})).status, 403);
  assert.equal((await (await fetch(`${url}/v1/models`,{headers})).json()).data[1].id, 'gpt-6.1-sol');
  const base = {model:'gpt-6.1-sol', messages:msg('hello'), tool_choice:null};
  const first = await (await request(base)).json();
  assert.equal(first.choices[0].message.content, 'fixture response');
  const second = await (await request({...base,messages:[...base.messages,first.choices[0].message,{role:'user',content:'continue'}]})).json();
  assert.equal(second.choices[0].message.content, 'fixture response');
  const stream = await (await request({...base,messages:msg('stream'),stream:true})).text();
  assert.match(stream,/fixture response/); assert.match(stream,/\[DONE\]/);
  const tools = ['a','b'].map(name=>({type:'function',function:{name,parameters:{type:'object'}}}));
  const messages = msg('two-tools private-prompt-sentinel');
  const output = await (await request({messages,tools})).json();
  const calls = output.choices[0].message.tool_calls;
  assert.equal(calls.length,2);
  const history = [...messages,output.choices[0].message];
  const response = call => ({role:'tool',tool_call_id:call.id,content:'ok'});
  const mismatch = await request({messages:[...history,...calls.map(response)],tools:tools.slice(0,1)});
  assert.equal(mismatch.status,409);
  assert.equal((await request({messages:[...history,response(calls[0])],tools})).status,400);
  const finished = await (await request({messages:[...history,...calls.map(response)],tools})).json();
  assert.equal(finished.choices[0].message.content,'tools complete');
  // Streaming clients can rewrite assistant content when returning tool output.
  const streamedMessages = msg('two-tools streamed-content-rewrite');
  const toolStream = await (await request({messages:streamedMessages,tools,stream:true})).text();
  const toolChunks = toolStream.split('\n').filter(s => s.startsWith('data: {')).map(s => JSON.parse(s.slice(6)));
  const streamedCalls = toolChunks.flatMap(chunk => chunk.choices?.[0]?.delta?.tool_calls || []).map(({index,...call})=>call);
  assert.equal(streamedCalls.length,2);
  const changedAssistant = {role:'assistant',content:'client rewrote streamed content',tool_calls:streamedCalls};
  const changedHistory = [...streamedMessages,changedAssistant];
  const results = streamedCalls.map(response);
  const changedTool = structuredClone(changedAssistant);
  changedTool.tool_calls[0].function.arguments = '{"modified":true}';
  assert.equal((await request({messages:[...streamedMessages,changedTool,...results],tools})).status,409);
  assert.equal((await request({messages:[...changedHistory,{...results[0],tool_call_id:'unknown'},results[1]],tools})).status,409);
  assert.equal((await request({messages:[...msg('different earlier history'),changedAssistant,...results],tools})).status,409);
  const resumed = await (await request({messages:[...changedHistory,...results],tools,stream:true})).text();
  assert.match(resumed,/tools complete/); assert.match(resumed,/\[DONE\]/);
  const singleMessages = msg('one-tool original-case');
  const singleStream = await (await request({messages:singleMessages,tools,stream:true})).text();
  assert.match(singleStream,/streamed text/);
  const singleChunks = singleStream.split('\n').filter(s => s.startsWith('data: {')).map(s => JSON.parse(s.slice(6)));
  const singleCalls = singleChunks.flatMap(chunk => chunk.choices?.[0]?.delta?.tool_calls || []).map(({index,...call})=>call);
  assert.equal(singleCalls.length,1);
  const clientHistory = [...singleMessages,{role:'assistant',content:null,tool_calls:singleCalls},response(singleCalls[0])];
  const singleResult = await (await request({messages:clientHistory,tools,stream:true})).text();
  assert.match(singleResult,/tools complete/); assert.match(singleResult,/\[DONE\]/);
  // History reconstructed after the bridge no longer has the original session.
  const oldHistory = [
    {role:'user',content:'older task'},
    {role:'assistant',content:null,tool_calls:[{id:'old_call',type:'function',function:{name:'a',arguments:'{}'}}]},
    {role:'tool',tool_call_id:'old_call',content:'old result'},
    {role:'assistant',content:'finished'},
    {role:'user',content:'continue the old task'}
  ];
  const replay = await request({messages:oldHistory,tools});
  assert.equal(replay.status,200);
  assert.equal((await replay.json()).choices[0].message.content,'fixture response');
  const unresolved = await request({messages:oldHistory.slice(0,3),tools});
  assert.equal(unresolved.status,409);
  const failed = await request({messages:msg('fail-turn')});
  assert.equal(failed.status,502); assert.match(JSON.stringify(await failed.json()),/fixture failure/);
  const rawLogs = readFileSync(diagnosticFile,'utf8');
  assert(!rawLogs.includes('private-prompt-sentinel'));
  assert(!rawLogs.includes('smoke-test-token-0123456789abcdef'));
  const events = rawLogs.trim().split('\n').map(JSON.parse);
  assert(events.some(e => e.event === 'session_not_found'));
  assert(events.some(e => e.event === 'session_lookup' && e.data.candidates.some(c => c.same_model && !c.same_tools && c.matching_tool_result_count === 2)));
  assert(events.some(e => e.event === 'response_ready' && e.data.tool_call_count === 2));
  assert(events.some(e => e.event === 'session_lookup' && e.data.match_kind === 'pending_tool'));
  console.log('Rust HTTP fixture: authentication, origins, models, continuation, SSE, parallel tools and errors passed.');
} finally {
  child.stdin.end('stop\n');
  await once(child,'exit');
  try { unlinkSync(diagnosticFile); } catch (error) { if (error.code !== 'ENOENT') throw error; }
}
