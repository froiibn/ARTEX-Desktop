// Validate source provenance and canonical contracts independently of runtime tests.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { requireOriginalRoot } from './source-layout.mjs';
const root = path.resolve(import.meta.dirname, '..'), dir = path.join(root,'docs/contracts');
const originalRoot = requireOriginalRoot();
const require = createRequire(path.join(originalRoot,'web/package.json'));
const Ajv = require('ajv/dist/2020').default, addFormats = require('ajv-formats');
const ajv = new Ajv({ strict:false, allErrors:true }); addFormats(ajv);
ajv.addKeyword({keyword:'x-max',type:'string',schemaType:'string',validate:(max,value)=>{try{return BigInt(value)<=BigInt(max);}catch{return false;}}});
ajv.addKeyword({keyword:'x-range',type:'string',schemaType:'string',validate:(range,value)=>{try{const n=BigInt(value);return range==='int64' && n>=-9223372036854775808n && n<=9223372036854775807n;}catch{return false;}}});
const read = n => JSON.parse(fs.readFileSync(path.join(dir,n),'utf8'));
const sha = b => crypto.createHash('sha256').update(b).digest('hex');
const unique = (items,name) => assert.equal(new Set(items).size,items.length,name+' must be unique');
const model = read('data-model.v1.json'), tools = read('tools.v1.json'), baseline = read('tools-source.v1.json');
const registry = read('legacy-routes.v1.json'), ui = read('ui-dtos.v1.json'), rpc = read('rpc.v1.schema.json');
const entities = read('entities.v1.schema.json'), storage = read('storage.v1.schema.json'), agent = read('agent-message.v1.schema.json');
const collections = read('collection-registry.v1.json'), examples = read('examples.v1.json');
assert.deepEqual(entities.$defs.finding_retests.properties.data.properties.verdict.enum,['','reproduced','fixed','inconclusive'],'SQL CHECK enum must preserve empty SQL strings');
assert.equal(entities.$defs.traffic_evidence_snapshots.properties.data.properties.id.pattern,'^evidence_[0-9a-f]{32}$','Evidence snapshot primary key uses its new EntityId');
assert.deepEqual(entities.$defs.side_question_requests.properties.data.properties.session_key,{type:'string'},'FKs to natural keys must keep the natural key type');
assert.equal(model.tables.length,54); assert.equal(model.tables.reduce((n,t)=>n+t.columns.length,0),571);
unique(model.tables.map(t=>t.name),'tables');
for (const t of model.tables) {
 unique(t.columns.map(c=>c.name),t.name+' columns');
 assert.ok(entities.$defs[t.name],t.name+' entity schema');
 assert.equal(Object.keys(entities.$defs[t.name].properties.data.properties).length,t.columns.length);
 for (const c of t.columns) { assert.ok(c.desktop_schema,t.name+'.'+c.name); if (c.reference) assert.ok(model.tables.some(x=>x.name===c.reference.collection)); }
}
assert.equal(model.tables.find(t=>t.name==='agents').columns.find(c=>c.name==='current_prompt_id').reference.collection,'agent_prompts');
assert.equal(registry.routes.length,261); unique(registry.routes.map(r=>r.route_id),'route IDs');
unique(registry.routes.map(r=>r.legacy_http_method+' '+r.legacy_path),'HTTP routes');
assert.equal(registry.routes.filter(r=>r.registration==='server/finding_traffic.go').length,7);
assert.equal(registry.routes.filter(r=>r.registration==='server/side_questions.go').length,11);
for (const r of registry.routes) {
 assert.ok(fs.existsSync(path.join(originalRoot,r.file)),r.file);
 assert.match(r.go_contract,new RegExp('func \\(s \\*Server\\) '+r.handler+'\\('));
 if (r.disposition==='removed_os_session') assert.ok(!rpc.methods[r.rpc_method]);
 else assert.ok(rpc.methods[r.rpc_method].properties.route_id.enum.includes(r.route_id),r.route_id);
}
assert.equal(ui.operations.length,258); unique(ui.operations.map(o=>o.method),'UI operations');
for (const op of ui.operations) { assert.ok(rpc.methods[op.method]); assert.ok(rpc.results[op.method]); }
assert.equal(tools.tools.length,81); assert.equal(baseline.length,81); unique(tools.tools.map(t=>t.name),'tools');
assert.equal(tools.tools.filter(t=>t.family==='artex').length,51); assert.equal(tools.tools.filter(t=>t.family==='sdk').length,30);
assert.deepEqual(tools.tools.filter(t=>t.desktop_status==='implement_opt_in_default_off').map(t=>t.name).sort(),['AskUserQuestion','EnterPlanMode','ExitPlanMode','RecallMemory','RecordMemory'].sort());
for (const t of tools.tools) {
 assert.ok(t.description_ko); assert.ok(t.rust_handler); assert.ok(fs.existsSync(path.join(originalRoot,t.source.file)));
 assert.deepEqual(t.input_schema,baseline.find(x=>x.name===t.name).input_schema);
}
for (const f of read('source-manifest.v1.json').files) assert.equal(sha(fs.readFileSync(path.join(originalRoot,f.path))),f.sha256,'source drift: '+f.path);
// Compile every standalone DTO and method with Draft 2020-12.
let schemas = 0;
const checkSchema = (s,defs={}) => { schemas++; return ajv.compile({$schema:'https://json-schema.org/draft/2020-12/schema',$defs:defs,...s}); };
for (const s of Object.values(entities.$defs)) checkSchema(s,entities.$defs);
for (const s of Object.values(storage.$defs)) checkSchema(s,storage.$defs);
for (const s of Object.values(rpc.methods)) checkSchema(s,rpc.$defs);
for (const s of Object.values(rpc.results)) checkSchema(s,rpc.$defs);
for (const t of tools.tools) { checkSchema(t.input_schema); checkSchema(t.desktop_input_schema); }
unique(collections.collections.map(x=>x.collection),'collection registry');
for (const c of collections.collections) if (c.kind!=='read_projection') assert.ok(entities.$defs[c.schema],c.collection+' schema');
function valid(s,value,defs,label) { const v=checkSchema(s,defs); assert.ok(v(value),label+': '+JSON.stringify(v.errors)); }
function invalid(s,value,defs,label) { const v=checkSchema(s,defs); assert.ok(!v(value),label+' unexpectedly accepted'); }
valid(storage.$defs.Project,examples.project,storage.$defs,'project');
valid(agent,examples.message,agent.$defs,'agent message');
valid(rpc.$defs.Request,examples.rpc_request,rpc.$defs,'RPC request');
valid(rpc.methods['agent.send'],examples.rpc_request.params,rpc.$defs,'agent.send params');
valid(rpc.$defs.Success,examples.rpc_success,rpc.$defs,'RPC success');
valid(rpc.results['agent.send'],examples.rpc_success.result,rpc.$defs,'agent.send result');
valid(rpc.$defs.Failure,examples.rpc_failure,rpc.$defs,'RPC failure');
valid(entities.$defs.companies,examples.company,entities.$defs,'company');
const report = tools.tools.find(t=>t.name==='update_finding_report');
valid(report.desktop_input_schema,examples.report_input,{},'report input');
invalid(report.desktop_input_schema,{...examples.report_input,finding_node_id:'finding_'+'1'.repeat(32)},{},'FindingId passed as NodeId');
const withoutVersion={...examples.report_input}; delete withoutVersion.evidence_version;
invalid(report.desktop_input_schema,withoutVersion,{},'report without evidence version');
invalid(commonRef('Body'),{text:'x',blob:{sha256:'0'.repeat(64),length:'1',media_type:'text/plain'}},entities.$defs,'two body representations');
invalid(rpc.$defs.Request,{...examples.rpc_request,timeout_ms:Infinity},rpc.$defs,'infinite timeout');
invalid(entities.$defs.UInt64,'9007199254740993.0',entities.$defs,'fractional sequence');
invalid(entities.$defs.UInt64,'18446744073709551616',entities.$defs,'uint64 overflow');
valid(entities.$defs.UInt64,'18446744073709551615',entities.$defs,'uint64 maximum');
valid(rpc.methods['ui.renameTaskCategory'],{id:'category_'+'1'.repeat(32),name:'분류'},rpc.$defs,'typed UI category');
invalid(rpc.methods['ui.renameTaskCategory'],{id:42,name:'분류'},rpc.$defs,'legacy numeric UI category');
valid(rpc.methods['ui.conversationMsgDetail'],{id:'conversation_'+'1'.repeat(32),seq:'conversationactivity_'+'1'.repeat(32)},rpc.$defs,'conversation activity identity');
invalid(rpc.methods['ui.conversationMsgDetail'],{id:'conversation_'+'1'.repeat(32),seq:'activity_'+'1'.repeat(32)},rpc.$defs,'task activity used in conversation');
// Interception histories can belong to a task or a conversation. Both the
// execution and each returned Activity must keep that exact identity union.
const interception=rpc.results['ui.interceptExecution'];
const interceptionDef=rpc.$defs[interception.$ref.split('/').at(-1)];
const interceptionActivity=rpc.$defs[interceptionDef.properties.items.items.$ref.split('/').at(-1)];
const exactActivityIds={anyOf:[{type:'string',pattern:'^activity_[0-9a-f]{32}$'},{type:'string',pattern:'^conversationactivity_[0-9a-f]{32}$'}]};
assert.deepEqual(interceptionActivity.properties.seq,exactActivityIds,'Nested interception Activity ID union is exact');
for(const prefix of ['activity','conversationactivity']) {
 const seq=prefix+'_'+'1'.repeat(32);
 const payload={conversation_id:prefix==='conversationactivity'?'conversation_'+'1'.repeat(32):null,task_id:prefix==='activity'?'task_'+'1'.repeat(32):null,session:'fixture',seq,items:[{seq,worker:'fixture',ts:'2026-10-13T00:00:00Z',kind:'text',summary:'fixture'}]};
 valid(interception,payload,rpc.$defs,'interception '+prefix+' execution and nested activity identity');
 for(const invalidSeq of ['node_'+'1'.repeat(32),'finding_'+'1'.repeat(32),'activity_'+'g'.repeat(32),42]) {
  invalid(interception,{...payload,items:[{...payload.items[0],seq:invalidSeq}]},rpc.$defs,'interception rejects unrelated or malformed nested identity');
  invalid(interception,{...payload,seq:invalidSeq},rpc.$defs,'interception rejects unrelated or malformed parent identity');
 }
}
const interceptionOverride=read('ui-wire-overrides.v1.json').identity_fields.filter(field=>field.method==='ui.interceptExecution'&&field.role==='result'&&field.field==='seq');
assert.equal(interceptionOverride.length,1);
assert.equal(interceptionOverride[0].prefix,'activity|conversationactivity');
valid(rpc.methods['ui.updateTaskLLMProfiles'],{id:'task_'+'1'.repeat(32),llmProfileIds:['profile_'+'1'.repeat(32)],activeLLMProfileId:null},rpc.$defs,'nullable active profile');
for (const enabled of [false,true]) {
 valid(rpc.methods['ui.setSettings'],{patch:{approval_bypass:enabled}},rpc.$defs,'workspace approval bypass bool');
 const settings={traffic_capture:false,agent_traffic_binding:false,llm_record:false,web_search_enabled:false,web_search_backend:'ddgs',brave_key_set:false,tavily_key_set:false,approval_bypass:enabled};
 for (const method of ['ui.settings','ui.setSettings']) valid(rpc.results[method],settings,rpc.$defs,method+' workspace approval bypass projection');
}
for(const value of ['true',1,null]) invalid(rpc.methods['ui.setSettings'],{patch:{approval_bypass:value}},rpc.$defs,'bypass rejects nonboolean');
invalid(rpc.methods['ui.setSettings'],{patch:{approval_bypas:true}},rpc.$defs,'bypass rejects misspelled settings');
function commonRef(n) { return {$ref:'#/$defs/'+n}; }
const vector = read('journal-vector.v1.json'), frame=Buffer.from(vector.frame_hex,'hex');
assert.equal(frame.subarray(0,8).toString(),'ARTEXJ01'); assert.equal(frame.readUInt16LE(8),1); assert.equal(frame.readUInt16LE(10),0);
assert.equal(frame.readUInt32LE(12),vector.payload_length); assert.equal(frame.readBigUInt64LE(16).toString(),vector.seq);
assert.equal(frame.subarray(40,72).toString('hex'),vector.previous_frame_sha256);
assert.equal(frame.length,104+vector.payload_length+8); assert.equal(frame.length,vector.frame_length);
assert.equal(frame.subarray(-8).toString(),'ARTEXEND'); assert.equal(sha(frame),vector.frame_sha256);
const payload=frame.subarray(104,-8); assert.equal(sha(payload),frame.subarray(72,104).toString('hex'));
assert.deepEqual(JSON.parse(payload),vector.payload); valid(storage.$defs.JournalPayload,vector.payload,storage.$defs,'journal payload');
const corrupt=Buffer.from(frame); corrupt[104]^=1; assert.notEqual(sha(corrupt.subarray(104,-8)),corrupt.subarray(72,104).toString('hex'));
assert.ok(frame.subarray(0,-1).length<104+vector.payload_length+8);
for (const file of ['docs/desktop-architecture.md','docs/desktop-transition-plan.md','docs/desktop-implementation-spec.md','docs/desktop-tool-mapping.md']) {
 const text=fs.readFileSync(path.join(root,file),'utf8');
 for (const m of text.matchAll(/\]\(([^)]+)\)/g)) { const target=m[1].split('#')[0]; if(!target||/^[a-z]+:/i.test(target)) continue; assert.ok(fs.existsSync(path.resolve(root,path.dirname(file),target)),file+' broken link: '+target); }
}
assert.ok(!fs.existsSync(path.join(originalRoot,'server/desktop_contract_dump_test.go')),'temporary exporter must be removed');
console.log(`Contract validation passed: 54 tables / 571 fields, 261 routes, 258 UI operations, 81 tools; ${schemas} schemas compiled, positive/negative examples and journal bytes verified.`);
