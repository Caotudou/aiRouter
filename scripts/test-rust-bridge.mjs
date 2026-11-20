import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { createInterface } from 'node:readline';
import assert from 'node:assert/strict';
import { resolve } from 'node:path';
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, unlinkSync } from 'node:fs';

const suffix = process.platform === 'win32' ? '.exe' : '';
const dir = resolve('src-tauri/target/debug/examples');
mkdirSync(resolve('.local'), {recursive:true});
const diagnosticFile = resolve('.local', `diagnostics-test-${process.pid}.jsonl`);
const usageDir = mkdtempSync(resolve('.local', 'usage-test-'));
const child = spawn(`${dir}/server_smoke${suffix}`, [`${dir}/fake_codex${suffix}`], { stdio: ['pipe', 'pipe', 'inherit'], windowsHide: true, env:{...process.env, AI_ROUTER_DIAGNOSTICS_PATH:diagnosticFile, AI_ROUTER_TEST_USAGE_DIR:usageDir} });
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
  const usageMessages = msg('one-tool usage-fixture do-not-store-this-prompt');
  const usageTools = [{ type: 'function', function: { name: 'read', parameters: { type: 'object' } } }];
  const firstUsage = await (await request({ messages: usageMessages, tools: usageTools, stream: true })).text();
  const usageCall = firstUsage.split('\n').filter(line => line.startsWith('data: {'))
    .map(line => JSON.parse(line.slice(6))).flatMap(chunk => chunk.choices[0].delta.tool_calls || [])
    .map(({index, ...call}) => call)[0];
  assert.ok(usageCall);
  const resumedUsage = await (await request({
    messages: [...usageMessages, { role: 'assistant', content: 'streamed text', tool_calls: [usageCall] }, { role: 'tool', tool_call_id: usageCall.id, content: 'ok' }],
    tools: usageTools,
  })).json();
  assert.equal(resumedUsage.choices[0].message.content, 'tools complete');
  assert.equal((await request({messages:msg('usage-fixture standalone')})).status, 200);
  assert.equal((await request({messages:msg('usage-fixture self-test')}, {headers:{...headers, 'x-ai-router-connection-test':'1'}})).status, 200);
  const rawUsage = readdirSync(usageDir).map(file => readFileSync(resolve(usageDir, file), 'utf8')).join('\n');
  const usageRecords = rawUsage.split('\n').filter(Boolean).map(JSON.parse);
  assert.equal(usageRecords.filter(record => record.type === 'request').length, 3);
  const tokenRecords = usageRecords.filter(record => record.type === 'tokens');
  assert.equal(tokenRecords.length, 3);
  assert.equal(tokenRecords.reduce((sum, record) => sum + record.tokens.total, 0), 74);
  assert.equal(tokenRecords.reduce((sum, record) => sum + record.tokens.cachedInput, 0), 18);
  assert.ok(!rawUsage.includes('do-not-store-this-prompt') && !rawUsage.includes(headers.authorization));
  child.stdin.write(JSON.stringify({model:'selected-model',effort:'medium'})+'\n');
  assert.equal((await once(lines,'line'))[0], 'DEFAULTS_UPDATED');
  const selected = await (await request({model:'codex-local',messages:msg('inspect-settings selected')})).json();
  assert.equal(selected.model,'selected-model');
  assert.equal(selected.choices[0].message.content,'model=selected-model;effort=medium');
  const override = await (await request({model:'client-model',reasoning_effort:'high',messages:msg('inspect-settings override')})).json();
  assert.equal(override.choices[0].message.content,'model=client-model;effort=high');
  const nested = await (await request({reasoning:{effort:'xhigh'},messages:msg('inspect-settings nested')})).json();
  assert.equal(nested.choices[0].message.content,'model=selected-model;effort=xhigh');
  const selectedStream = await (await request({model:'codex-local',messages:msg('inspect-settings stream'),stream:true})).text();
  assert.match(selectedStream,/"model":"selected-model"/);
  assert.match(selectedStream,/effort=medium/);
  child.stdin.write(JSON.stringify({model:'gpt-6.1-sol',effort:'high'})+'\n');
  assert.equal((await once(lines,'line'))[0], 'DEFAULTS_UPDATED');
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
  assert.equal((await request({messages:[...history,response(calls[0])],tools})).status,400);
  const finished = await (await request({messages:[...history,...calls.map(response)],tools:tools.slice(0,1)})).json();
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
  const resumed = await (await request({messages:[...msg('different earlier history'),changedTool,...results],tools,stream:true})).text();
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
  // Reproduce WorkBuddy sending a tool result and a new user message together.
  const combinedMessages = msg('one-tool with-follow-up');
  const combinedStart = await (await request({messages:combinedMessages,tools})).json();
  const combinedCall = combinedStart.choices[0].message.tool_calls[0];
  const combinedHistory = [...combinedMessages,combinedStart.choices[0].message];
  const followUp = {role:'user',content:'new user instruction'};
  assert.equal((await request({messages:[...combinedHistory,{...response(combinedCall),tool_call_id:'unknown'},followUp],tools})).status,400);
  const combinedResult = await (await request({messages:[...combinedHistory,response(combinedCall),followUp],tools,stream:true})).text();
  assert.match(combinedResult,/user instruction received/); assert.match(combinedResult,/\[DONE\]/);
  // Also match a changed streamed assistant content when the user follows the tool result.
  const changedStart = await (await request({messages:msg('one-tool with-rewrite-follow-up'),tools})).json();
  const changedCall = changedStart.choices[0].message.tool_calls[0];
  const changedMessage = {...changedStart.choices[0].message,content:null};
  const changedCombined = await (await request({messages:[...msg('one-tool with-rewrite-follow-up'),changedMessage,response(changedCall),followUp],tools})).json();
  assert.match(changedCombined.choices[0].message.content,/user instruction received/);
  // WorkBuddy may omit the steered user message in its next history snapshot.
  const chainMessages = msg('chain-tools history reshaped');
  const chainStart = await (await request({messages:chainMessages,tools})).json();
  const chainCall = chainStart.choices[0].message.tool_calls[0];
  const firstChainHistory = [...chainMessages,chainStart.choices[0].message,response(chainCall)];
  const chainNext = await (await request({messages:[...firstChainHistory,followUp],tools})).json();
  const nextCall = chainNext.choices[0].message.tool_calls[0];
  const reshapedHistory = [...firstChainHistory,chainNext.choices[0].message,response(nextCall)];
  const chainDone = await (await request({messages:reshapedHistory,tools,stream:true})).text();
  assert.match(chainDone,/user instruction received/); assert.match(chainDone,/\[DONE\]/);
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
  const recovered = await request({messages:oldHistory.slice(0,3),tools});
  assert.equal(recovered.status,200);
  const custom = await request({messages:msg('custom model passthrough'),model:'not-in-catalog',reasoning_effort:'custom'});
  assert.equal(custom.status,200);
  const failed = await request({messages:msg('fail-turn')});
  assert.equal(failed.status,502); assert.match(JSON.stringify(await failed.json()),/fixture failure/);
  const nativeTool = await request({messages:msg('native-tool-fixture')});
  assert.equal(nativeTool.status, 502);
  assert.equal((await nativeTool.json()).error.code, 'native_tool_blocked');
  const rawLogs = readFileSync(diagnosticFile,'utf8');
  assert(!rawLogs.includes('private-prompt-sentinel'));
  assert(!rawLogs.includes('smoke-test-token-0123456789abcdef'));
  assert(!rawLogs.includes('omit-native-tool-arguments'));
  const events = rawLogs.trim().split('\n').map(JSON.parse);
  assert(events.some(e => e.event === 'session_lookup' && e.data.matched && e.data.candidates.some(c => c.same_model && !c.same_tools && c.matching_tool_result_count === 2)));
  assert(events.some(e => e.event === 'response_ready' && e.data.tool_call_count === 2));
  assert(events.some(e => e.event === 'session_lookup' && e.data.match_kind === 'pending_tool'));
  assert(events.some(e => e.event === 'turn_steered' && e.data.user_message_count === 1));
  assert(events.some(e => e.event === 'native_tool_blocked' && e.data.item_type === 'mcpToolCall'));
  console.log('Rust HTTP fixture: authentication, origins, models, continuation, SSE, usage, parallel tools and errors passed.');
} finally {
  child.stdin.end('stop\n');
  await once(child,'exit');
  try { unlinkSync(diagnosticFile); } catch (error) { if (error.code !== 'ENOENT') throw error; }
  rmSync(usageDir, {recursive:true, force:true});
}
