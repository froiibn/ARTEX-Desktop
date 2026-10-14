// Normative ARTEX desktop v1 contracts and the complete tool migration matrix.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import {extendWorkspaceApprovalSettings} from './native-settings-contract.mjs';
import { requireOriginalRoot } from './source-layout.mjs';
const root = path.resolve(import.meta.dirname, '..'), dir = path.join(root, 'docs/contracts');
const originalRoot = requireOriginalRoot();
const read = p => JSON.parse(fs.readFileSync(path.join(dir, p), 'utf8'));
const save = (p, data) => fs.writeFileSync(path.join(dir, p), JSON.stringify(data, null, 2) + '\n');
const dialect = 'https://json-schema.org/draft/2020-12/schema';
const str = { type: 'string' }, bool = { type: 'boolean' };
const uint = { type: 'string', pattern: '^(0|[1-9][0-9]*)$', 'x-max': '18446744073709551615' };
const timestamp = { type: 'string', format: 'date-time', pattern: 'Z$' };
const digest = { type: 'string', pattern: '^[0-9a-f]{64}$' };
const id = prefix => ({ type: 'string', pattern: `^${prefix}_[0-9a-f]{32}$` });
const obj = (properties, required = Object.keys(properties)) => ({ type: 'object', properties, required, additionalProperties: false });
const array = items => ({ type: 'array', items });
const nullable = s => ({ anyOf: [s, { type: 'null' }] });
const ref = name => ({ $ref: `#/$defs/${name}` });
const prefixes = {
 companies:'company', assets:'asset', company_scope:'companyscope', explorations:'exploration', exploration_nodes:'node', exploration_edges:'edge', exploration_anchors:'anchor', task_constraints:'constraint', activity:'activity', main_sessions:'mainsession', settings:'setting', llm_profiles:'profile', llm_profile_health:'profilehealth', task_categories:'category', tasks:'task', task_archives:'archive', task_templates:'template', task_relations:'relation', task_asset_links:'assetlink', task_llm_profiles:'profilelink', task_scope:'scope', agents:'agent', agent_prompts:'prompt', agent_prompt_vars:'promptvar', mcp_servers:'mcp', mcp_tools_cache:'mcptool', agent_visibility:'visibility', agent_skill_visibility:'skillvisibility', skill_usage:'skillusage', tool_usage:'toolusage', tools:'toolconfig', conversations:'conversation', conversation_activities:'conversationactivity', agent_triggers:'trigger', scheduler_state:'scheduler', intercept_rules:'interceptrule', intercept_pending:'approval', findings:'finding', finding_retests:'retest', traffic_evidence_snapshots:'evidence', finding_traffic_bindings:'binding', server_logs:'log', side_question_sessions:'sidesession', side_question_requests:'sidequestion', asset_intercept_rules:'assetrule', task_intercept_rules:'taskrule', notification_channels:'notifychannel', notification_events:'notifyevent', notification_deliveries:'notifydelivery', llm_records:'llmrecord', llm_usage:'llmusage', exchanges:'traffic', exchange_bodies:'trafficbody', blob_refs:'bloblink'
};
const foreign = { task_id:'task', source_task_id:'task', exploration_id:'exploration', node_id:'node', src_id:'node', dst_id:'node', source_node_id:'node', asset_id:'asset', company_id:'company', category_id:'category', category_id_snapshot:'category', agent_id:'agent', current_prompt_id:'prompt', profile_id:'profile', llm_profile_id:'profile', active_llm_profile_id:'profile', finding_id:'finding', snapshot_id:'evidence', conversation_id:'conversation', server_id:'mcp', resource_id:'mcp', event_id:'notifyevent', channel_id:'notifychannel', batch_id:'notifydelivery', rule_id:'interceptrule' };
const common = {
 UInt64:uint, Timestamp:timestamp, Digest:digest,
 BlobRef:obj({ sha256:digest, length:uint, media_type:str }),
 SecretRef:obj({ secret_ref:id('secret'), hint:str }),
 Expected:obj({ collection:str,id:str, version:uint }),
 Error:obj({ code:{ type:'integer' }, message:str, data:obj({ code:str, retryable:bool, recovery_required:bool, details:{ type:'object' } }, ['code','retryable','recovery_required']) }),
 EntityHeader:obj({ id:str, collection:str, version:uint, commit_seq:uint, created_at:timestamp, updated_at:timestamp, legacy_key:nullable(str) }),
 Body:obj({ text:nullable(str), blob:nullable(ref('BlobRef')) }),
 Actor:obj({ kind:{enum:['user','engine','agent']},agent_id:nullable(id('agent')), run_id:nullable(id('run')), engine_epoch:id('epoch') }),
 Run:obj({ id:id('run'), agent_id:id('agent'), task_id:nullable(id('task')), intent_id:nullable(id('node')), conversation_id:nullable(id('conversation')), engine_epoch:id('epoch'), state:{ enum:['queued','running','waiting','paused','completed','failed','cancelled','interrupted','recovery_required'] }, started_at:nullable(timestamp), finished_at:nullable(timestamp), deadline_at:nullable(timestamp), next_tool_ordinal:uint }),
 DomainEvent:obj({ id:id('event'), seq:uint, kind:str, task_id:nullable(id('task')), subject_collection:str,subject_id:str, subject_version:uint, actor:ref('Actor'), created_at:timestamp }),
 AgentDelivery:obj({ id:id('delivery'), event_id:nullable(id('event')), message_id:nullable(id('message')), recipient_id:id('agent'), activation_key:str, state:{ enum:['pending','claimed','observed','completed','expired','cancelled','interrupted'] }, owner:nullable(ref('Actor')), claim_deadline:nullable(timestamp), attempts:{ type:'integer', minimum:0 }, observed_at:nullable(timestamp), completed_at:nullable(timestamp) }),
 SteeringMessage:obj({ id:id('steering'), task_id:id('task'), intent_id:id('node'), target_run_id:id('run'), text:str, ordinal:uint, state:{ enum:['pending','consumed','dropped'] }, created_at:timestamp }),
 MemoryRecord:obj({ id:id('memory'), agent_id:id('agent'), name:str, description:str, type:{ enum:['user','feedback','project','reference'] }, content:ref('Body'), version:uint, created_at:timestamp, updated_at:timestamp }),
 ToolInvocation:obj({ id:id('invocation'), run_id:id('run'), tool_use_id:str, ordinal:uint, name:str, input:{}, raw_input:nullable(str), state:{ enum:['prepared','awaiting_approval','running','completed','failed','cancelled','recovery_required'] }, execution_id:nullable(id('exec')), input_ref:nullable(ref('BlobRef')), result_ref:nullable(ref('BlobRef')), created_at:timestamp }),
 CommandReceipt:obj({ command_id:id('command'), method:str, normalized_params:{}, expected:array(ref('Expected')), commit_seq:uint, result:{}, result_ref:nullable(ref('BlobRef')) }),
 UserQuestion:obj({ id:id('question'), run_id:id('run'), kind:{ enum:['question','plan_approval'] }, payload:{ type:'object' }, state:{ enum:['pending','answered','cancelled','expired'] }, answer:nullable({}), deadline_at:timestamp }),
 ResourceFile:obj({ resource_kind:{ enum:['skill','mcp_resource'] }, resource_key:str, relative_path:str, blob:ref('BlobRef'), revision:uint }),
 Execution:obj({ id:id('exec'), invocation_id:id('invocation'), owner:ref('Actor'), state:{ enum:['prepared','running','completed','failed','cancelled','unknown'] }, program_capability:str, args:array(str), cwd_capability:str, started_at:nullable(timestamp), finished_at:nullable(timestamp), exit_code:nullable({ type:'integer' }), output_ref:nullable(ref('BlobRef')), last_cursor:uint }),
 ShellSession:obj({ id:id('shell'), execution_id:id('exec'), run_id:id('run'), state:{ enum:['running','closed','interrupted'] }, cursor:uint, rows:{ type:'integer', minimum:1 }, cols:{ type:'integer', minimum:1 } }),
 Tombstone:obj({ collection:str,id:str,version:uint,commit_seq:uint,deleted_at:timestamp,legacy_key:nullable(str) }),
 AgentFeatures:obj({ agent_id:id('agent'),questions:bool,plan_mode:bool,memory:bool }),
 TaskRuntime:obj({task_id:id('task'),timeout_policy:{enum:['active_elapsed','legacy_absolute']},active_elapsed_ms:uint,remaining_timeout_ms:nullable(uint),last_active_clock:nullable(uint),legacy_deadline_at:nullable(timestamp),accounting_state:{enum:['known','recovery_required']}}),
};
common.ContentBlock = { oneOf:[
 obj({type:{const:'text'},text:ref('Body')}),
 obj({type:{const:'thinking'},thinking:ref('Body'),signature:str}),
 obj({type:{const:'tool_use'},id:str,name:str,input:{}}),
 obj({type:{const:'tool_result'},tool_use_id:str,content:array(ref('ContentBlock')),is_error:bool})
] };
common.ConversationMessage = obj({role:{enum:['user','assistant']},content:array(ref('ContentBlock'))});
common.ToolResult = obj({content:array(ref('ContentBlock')),is_error:bool,extra:array(ref('ConversationMessage'))});
const permissionMode = { enum:['default','acceptEdits','bypassPermissions','plan','dontAsk','auto'] };
common.Run.properties.permission_mode = permissionMode;
common.Run.properties.pre_plan_mode = nullable(permissionMode);
common.Run.properties.approved_target_mode = permissionMode;
common.Run.required.push('permission_mode','pre_plan_mode','approved_target_mode');
common.Run.properties.todos = array(obj({content:str,status:{enum:['pending','in_progress','completed']}}));
common.Run.required.push('todos');
common.Run.properties.purpose={enum:['normal','run_wrapup','task_timeout_wrapup']};
common.DomainEvent.properties.capture=obj({state:str,error:nullable(str),request_captured_bytes:uint,response_captured_bytes:uint});
common.Run.properties.parent_run_id=nullable(id('run'));
common.Run.properties.turn_count=uint;
common.Run.properties.context_snapshot={type:'object',properties:{skill_catalog_names:{type:'array',items:{type:'string'}},skill_catalog:{type:'array',items:{type:'object',properties:{name:{type:'string'},description:{type:['string','null']},mcps:{anyOf:[{type:'string'},{type:'array',items:{type:'string'}},{type:'null'}]}},required:['name','description','mcps'],additionalProperties:false}},skill_catalog_error:{type:'string'}},additionalProperties:false};
common.UserQuestion.properties.generation = uint; common.UserQuestion.required.push('generation');
common.Body.oneOf = [obj({ text:str, blob:{ type:'null' } }), obj({ text:{ type:'null' }, blob:ref('BlobRef') })];
const model = read('data-model.v1.json'), definitions = { ...common };
for (const t of model.tables) {
  t.id_prefix = prefixes[t.name];
  const properties = {};
  for (const c of t.columns) {
    let s;
    const naturalReference = c.reference && c.reference.field !== 'id';
    const f = c.reference ? naturalReference ? null : prefixes[c.reference.collection] : ['llm_records','llm_usage'].includes(t.name) && c.name === 'task_id' ? null : t.name === 'side_question_sessions' && c.name === 'run_id' ? 'run' : foreign[c.name];
    if (naturalReference) { c.desktop_type = 'string'; c.migration = 'preserve_natural_foreign_key'; }
    if(t.name==='notification_events' && c.name==='finding_id'){c.desktop_type='typed_id';c.migration='map_historical_finding_provenance';}
    if (c.name === 'id') {
      const typed = /SERIAL/.test(c.sql_type) || ['traffic_evidence_snapshots','side_question_requests'].includes(t.name);
      s = typed ? id(prefixes[t.name]) : str;
      if (typed) { c.desktop_type = 'typed_id'; c.migration = 'map_legacy_entity_id'; }
    }
    else if (naturalReference) s = str;
    else if (f) s = id(f);
    else if (c.desktop_type === 'typed_id[]') s = array(id(c.name === 'asset_ids' ? 'asset' : c.name === 'member_ids' ? 'node' : 'task'));
    else {
      const base = c.desktop_type.replace('[]', '');
      s = base === 'decimal_string' ? { type:'string', pattern:'^(0|-?[1-9][0-9]*)$', 'x-range':'int64' } : base === 'int32' ? { type:'integer', minimum:-2147483648, maximum:2147483647 } : base === 'boolean' ? bool : base === 'finite_number' ? { type:'number' } : base === 'json_value' ? {} : base === 'utc_timestamp' ? timestamp : base === 'blob_ref' ? ref('BlobRef') : str;
      if (c.desktop_type.endsWith('[]')) s = array(s);
    }
    if (c.migration === 'split_secret_references') s = c.name === 'api_key' ? nullable(ref('SecretRef')) : c.name === 'auth' ? array({ type:'object', 'x-secret-leaves':'SecretRef' }) : { type:'object', 'x-secret-leaves':'SecretRef' };
    if (['detail','request_body','response_body','raw_request','raw_response','req_head','resp_head','report','evidence','answer','template_text','wrapup_prompt','task_timeout_wrapup_prompt'].includes(c.name)) { s = ref('Body'); c.storage_overlay = 'text_or_blob'; }
    const declarations = [c.declaration,...model.additional_constraints.filter(x=>x.collection===t.name).map(x=>x.declaration)].join(' ');
    const enums = [...declarations.matchAll(new RegExp(`CHECK\\s*\\(\\s*${c.name}\\s+IN\\s*\\(([^)]+)\\)`, 'gi'))];
    if (enums.length && s.type === 'string') s = {...s,enum:[...enums.at(-1)[1].matchAll(/'((?:[^']|'')*)'/g)].map(m=>m[1].replaceAll("''", "'"))};
    const range = declarations.match(new RegExp(`CHECK\\s*\\(\\s*${c.name}\\s+BETWEEN\\s+(\\d+)\\s+AND\\s+(\\d+)`, 'i'));
    if (range && s.type === 'integer') s = {...s,minimum:Number(range[1]),maximum:Number(range[2])};
    if (t.name==='findings' && c.name==='status') s={...s,enum:['pending','in_progress','confirmed','resolved','fixed','false_positive','ignored','duplicate','risk_accepted']};
    if (t.name==='findings' && c.name==='severity') s={...s,enum:['critical','high','medium','low']};
    if (c.nullable) s = nullable(s);
    properties[c.name] = s; c.desktop_schema = s;
  }
  definitions[t.name] = obj({ header:{ ...ref('EntityHeader') }, data:obj(properties) });
  definitions[t.name].properties.header = obj({ ...common.EntityHeader.properties, id:id(prefixes[t.name]), collection:{ const:t.name } });
}
save('data-model.v1.json', model);
const agentMessage = obj({ protocol:{ const:'artex-agent/1.0' }, message_id:id('message'), command_id:id('command'), workspace_id:id('workspace'), kind:{ enum:['send','request','reply','report','receipt','broadcast'] }, sender:ref('Actor'), recipients:array(id('agent')), channel:str, topic:str, correlation_id:nullable(id('message')), created_at:timestamp, deadline_at:nullable(timestamp), body:ref('Body'), object_refs:array(obj({ collection:str,id:str, version:uint, access:{ const:'read' } })), final:bool, outcome:{ enum:['none','ok','error','cancelled'] }, hop_count:{ type:'integer', minimum:0, maximum:16 } });
agentMessage.properties.sender = obj({kind:{const:'agent'},agent_id:id('agent'),run_id:id('run'),engine_epoch:id('epoch')});
Object.assign(agentMessage.properties.recipients,{minItems:1,maxItems:64,uniqueItems:true});
const request = obj({ id:id('message'), sender_id:id('agent'), recipients:array(id('agent')), expected_reports:array(id('agent')), state:{ enum:['open','claimed','waiting','completed','failed','cancelled','expired','interrupted'] }, terminal_reports:{ type:'object', additionalProperties:id('message') }, deadline_at:timestamp, completion_mode:{ enum:['one','all'] } });
definitions.AgentMessage = agentMessage;
definitions.ConversationRequest = request;
definitions.RecordEnvelope = obj({ header:ref('EntityHeader'), data:{} });
save('entities.v1.schema.json', { $schema:dialect, $id:'urn:artex:entities:1', $defs:definitions });
save('agent-message.v1.schema.json', { $schema:dialect, $id:'urn:artex:agent-message:1', ...agentMessage, $defs:{ ...common, ConversationRequest:request } });
const allCollections = [...Object.keys(prefixes), 'agent_runs','tool_invocations','domain_events','agent_messages','agent_deliveries','conversation_requests','steering_messages','command_receipts','user_questions','memory_records','resource_files','agent_features','task_runtime','traffic_records'];
const recordTypes = { agent_runs:'Run',tool_invocations:'ToolInvocation',domain_events:'DomainEvent',agent_messages:'AgentMessage',agent_deliveries:'AgentDelivery',conversation_requests:'ConversationRequest',steering_messages:'SteeringMessage',command_receipts:'CommandReceipt',user_questions:'UserQuestion',memory_records:'MemoryRecord',resource_files:'ResourceFile',agent_features:'AgentFeatures',task_runtime:'TaskRuntime' };
save('collection-registry.v1.json',{ version:'1.0', collections:[...model.tables.map(t => ({ collection:t.name, schema:t.name, id_prefix:t.id_prefix, source:t.source, kind:t.destination })), ...Object.entries(recordTypes).map(([collection,schema]) => ({ collection, schema, kind:'workspace_collection', id_prefix:{Run:'run',ToolInvocation:'invocation',DomainEvent:'event',AgentMessage:'message',AgentDelivery:'delivery',ConversationRequest:'message',SteeringMessage:'steering',CommandReceipt:'command',UserQuestion:'question',MemoryRecord:'memory',ResourceFile:'resource',AgentFeatures:'agentfeature',TaskRuntime:'taskruntime'}[schema] })), {collection:'traffic_records',kind:'read_projection',schema:'exchanges+exchange_bodies',id_prefix:'traffic'}] });
const methods = {
 'system.hello':obj({ versions:array(str), host_epoch:id('epoch'), capabilities:array(str) }),
 'system.ping':obj({}), 'system.shutdown':obj({ mode:{ enum:['pause','cancel'] } }),
 'workspace.create':obj({ name:str, directory_capability:str }), 'workspace.open':obj({ directory_capability:str, read_only:bool }),
 'workspace.info':obj({}), 'workspace.close':obj({}), 'workspace.checkpoint':obj({}),
 'workspace.recover':obj({ action:{ enum:['inspect','create_recovery_copy'] }, directory_capability:nullable(str) }),
 'query.objects':obj({ collection:{ enum:allCollections }, ids:array(str), cursor:nullable(str), limit:{ type:'integer', minimum:1, maximum:500 }, at_seq:nullable(uint) }),
 'command.status':obj({ command_id:id('command') }), 'command.prepare':obj({method:str,params:{}}), 'rpc.cancel':obj({ request_id:str }),
 'events.subscribe':obj({ after_seq:uint, kinds:array(str) }), 'events.unsubscribe':obj({ subscription_id:str }),
 'blob.begin':obj({ length:uint, media_type:str, sha256:digest }),
 'blob.write':obj({ transfer_id:id('transfer'), offset:uint, data_base64:str }),
 'blob.commit':obj({ transfer_id:id('transfer') }),
 'blob.read':obj({ sha256:digest, offset:uint, length:{ type:'integer', minimum:1, maximum:65536 } }),
 'task.recover_budget':obj({task_id:id('task'),mode:{enum:['use_recorded','override']},elapsed_active_ms:nullable(uint)}),
 'tool.invoke':obj({ run_id:id('run'), tool_use_id:str, name:str, input:{}, raw_input:nullable(str) }),
 'agent.send':obj({ message:agentMessage, request_options:nullable(obj({ completion_mode:{ enum:['one','all'] }, expected_reports:array(id('agent')) })) }), 'agent.inbox':obj({ agent_id:id('agent'), cursor:nullable(str), limit:{ type:'integer', minimum:1, maximum:100 } }),
 'agent.claim':obj({ delivery_id:id('delivery'), run_id:id('run') }),
 'agent.acknowledge':obj({ delivery_id:id('delivery'), run_id:id('run'), action:{ enum:['observed','completed'] } }),
 'agent.cancel_request':obj({ message_id:id('message') }),
 'user.answer':obj({ question_id:id('question'), answer:{}, generation:uint }),
 'agent.features.get':obj({agent_id:id('agent')}), 'agent.features.set':common.AgentFeatures,
 'host.secret.set':obj({ secret_ref:nullable(id('secret')), value:str, scope:{ enum:['workspace','app'] } }),
 'host.secret.delete':obj({ secret_ref:id('secret') }),
 'host.execute':obj({ invocation_id:id('invocation'), execution_id:id('exec'), program_capability:str, args:array(str), cwd_capability:str, env_refs:{ type:'object' }, timeout_ms:{ type:'integer', minimum:0 }, pty:bool }),
 'host.execution.status':obj({ execution_id:id('exec') }), 'host.execution.stop':obj({ execution_id:id('exec') }),
 'host.update.check':obj({}), 'host.update.apply':obj({ package_id:str }),
};
const ui = read('ui-dtos.v1.json');
// Produce v1 wire DTOs from the frozen legacy DTOs. Never validate typed IDs
// against the old frontend's number declarations.
const wireDefs = {}, wireCache = new Map(), wireOverrides = [];
const typeIds = {Task:'task',TaskCategory:'category',TaskTemplate:'template',TaskArchive:'archive',TaskGoal:'node',TaskConstraint:'constraint',TaskScopeRow:'scope',Asset:'asset',FindingAsset:'asset',Company:'company',ScopeRow:'companyscope',TaskNode:'node',Finding:'finding',FindingTrafficBinding:'binding',TrafficEvidenceSnapshot:'evidence',FindingRetest:'retest',ActiveFindingRetest:'retest',Conversation:'conversation',Agent:'agent',AgentTrigger:'trigger',PromptVersion:'prompt',PromptVar:'promptvar',LLMProfile:'profile',LLMAuditProfile:'profile',LLMPoolMember:'profile',MCPServer:'mcp',InterceptRule:'interceptrule',AssetInterceptRule:'assetrule',InterceptPending:'approval',InterceptApprovalRow:'approval',InterceptDetail:'approval',NotificationChannel:'notifychannel',NotificationDelivery:'notifydelivery',TrafficExchange:'traffic',CommandRecord:'activity',LLMRecordItem:'llmrecord',LLMRecordDetail:'llmrecord',SkillCall:'skillusage'};
const pluralIds = {task_ids:'task',source_task_ids:'task',asset_ids:'asset',parent_ids:'node',llm_profile_ids:'profile',company_ids:'company',archive_ids:'archive',binding_ids:'binding',goal_ids:'node',profile_ids:'profile',mcp:'mcp'};
const fieldIds = {...foreign,intent_id:'node',goal_id:'node',constraint_id:'constraint',binding_id:'binding',scope_id:'scope',context_task_id:'task',source_task_node_id:'node',finding_node_id:'node',traffic_id:'traffic'};
const childIds = {tasks:'task',assets:'asset',findings:'finding',nodes:'node',companies:'company',llm_profiles:'profile',profiles:'profile',categories:'category',templates:'template',bindings:'binding',rules:'interceptrule',channels:'notifychannel',deliveries:'notifydelivery',conversations:'conversation'};
function operationPrefix(method) {
 const s=method.slice(3);
 if (/TaskCategory|TaskCategories/i.test(s)) return 'category';
 if (/TaskTemplate/i.test(s)) return 'template';
 if (/TaskArchive/.test(s)) return 'archive';
 if (/AssetInterceptRule/.test(s)) return 'assetrule';
 if (/InterceptRule/.test(s)) return 'interceptrule';
 if (/intercept(GetOne|Decide|Execution|Detail)/.test(s)) return 'approval';
 if (/LLMProfile|LLMPool|LLMModels/.test(s)) return 'profile';
 if (/llmRecord/.test(s)) return 'llmrecord';
 if (/Conversation|conversation/.test(s)) return 'conversation';
 if (/Finding|finding/.test(s)) return 'finding';
 if (/Trigger/.test(s)) return 'trigger';
 if (/Company/.test(s)) return 'company';
 if (/Mcp|mcp|resourceVisibility/.test(s)) return 'mcp';
 if (/notify/.test(s)) return /Delivery/.test(s)?'notifydelivery':'notifychannel';
 if (/activity/i.test(s)) return 'activity';
 if (/deleteAssets|insertAssets|assets$/.test(s)) return 'asset';
 if (/Task|task|Intent|Goal|Constraint|rerun|setActive|chatUpload/.test(s)) return 'task';
 return null;
}
const snake = k=>k.replace(/([A-Z]+)([A-Z][a-z])/g,'$1_$2').replace(/([a-z0-9])([A-Z])/g,'$1_$2').toLowerCase();
function preserveNull(original,replacement) { return original.anyOf?.some(x=>x.type==='null')?nullable(replacement):replacement; }
function wire(s,ctx,key='',typeName='') {
 if(!s || typeof s!=='object') return s;
 if(s.$ref?.startsWith('#/$defs/T')) {
  const name=s.$ref.slice('#/$defs/'.length), cacheKey=name+':'+ctx.method+':'+ctx.prefix+':'+ctx.role;
  if(wireCache.has(cacheKey)) return {$ref:'#/$defs/'+wireCache.get(cacheKey)};
  const newName='W'+(wireCache.size+1); wireCache.set(cacheKey,newName); wireDefs[newName]={};
  const original=ui.$defs[name], sourceType=original['x-source-type']||'';
  const wrapped=sourceType.match(/^(?:Pick|Partial|Omit)<(\w+)/)?.[1];
  const exact=ctx.role==='result'&&/[Tt]askInterceptRule/.test(ctx.method)&&/AssetInterceptRule/.test(sourceType)?'taskrule':typeIds[sourceType]||typeIds[wrapped];
  const next={...ctx,prefix:exact||(sourceType.startsWith('{')?ctx.prefix:null)};
  wireDefs[newName]=wire(original,next,'',sourceType); return {$ref:'#/$defs/'+newName};
 }
 const k=snake(key);
 if(ctx.role==='params' && ['ui.workspaceWrite','ui.writeSkillFile'].includes(ctx.method)&&k==='content') return ref('Body');
 if(ctx.role==='params' && ctx.method==='ui.createSkill'&&k==='instructions') return preserveNull(s,ref('Body'));
 if(ctx.role==='result' && ctx.method==='ui.workspaceRead'&&k==='content') return ref('Body');
 if(ctx.role==='result' && ctx.method==='ui.readSkillFile'&&!key&&s.type==='string') return ref('Body');
 if(typeName==='AuditEntry'&&k==='ts') return {type:'integer'};
 if(typeName==='JudgeConfig'&&k==='profile_id') return nullable(id('profile'));
 if(typeName==='Asset'&&k==='content_length') return preserveNull(s,uint);
 if(typeName==='Asset'&&k==='task_source_node_id') return preserveNull(s,id('node'));
 if(ctx.role==='result' && ctx.method==='ui.ssDatasource' && k==='id') return id('mcp');
 if(ctx.role==='result' && typeName==='CommandRecord' && ['command','output'].includes(k)) return ref('Body');
 if(ctx.role==='result' && s['x-undefined'] && /ui\.(downloadFindingTrafficBody|exportFindings)$/.test(ctx.method)) return obj({content_base64:ref('Body'),mime:str,filename:str});
 if(ctx.role==='result' && ctx.method==='ui.report' && !key && s.type==='string') return ref('Body');
 if(ctx.role==='result' && ctx.method==='ui.trafficExchange' && ['req','resp'].includes(k)) return ref('Body');
 if(['version','report_version'].includes(k)&&typeName==='FindingTraffic') return uint;
 if(s['x-native-capability']) return ctx.role==='result' && ['File','Blob'].includes(s['x-native-capability'])?ref('BlobRef'):{...s,type:'string'};
 if(k==='api_key' || ['password','token'].includes(k) && !/login|initPassword|changePassword/.test(ctx.method)) return nullable(ref('SecretRef'));
 if(k==='env') return {type:'object',additionalProperties:{anyOf:[str,ref('SecretRef')]}};
 if(ctx.role==='result' && ['detail','request_body','response_body','raw_request','raw_response','req_head','resp_head','report','evidence','answer','template_text','wrapup_prompt','task_timeout_wrapup_prompt'].includes(k) && (s.type==='string'||s.anyOf?.some(x=>x.type==='string'))) return preserveNull(s,ref('Body'));
 if(k==='agents' && /resourceVisibility|skillVisibility/.test(ctx.method)) return array(id('agent'));
 let prefix=fieldIds[k];
 if(k==='task_id' && ['LLMRecordItem','LLMRecordDetail','LLMTask'].includes(typeName)) return {...s,'x-opaque-registry-key':true};
 if(k==='rule_id' && /[Tt]askInterceptRule/.test(ctx.method)) prefix='taskrule';
 if(k==='id' || k==='ids') prefix = /^SS/.test(typeName)?null:ctx.prefix;
 if(k==='id' && /updateTaskLLMProfiles/.test(ctx.method)) prefix='task';
 if(k==='id' && ctx.role==='result' && ctx.method==='ui.controlIntent') prefix='node';
 if(k==='deleted' && ctx.role==='result' && ctx.method==='ui.deleteTaskCategory') prefix='category';
 if(k==='deleted' && ctx.role==='result' && ctx.method==='ui.deleteTaskTemplate') prefix='template';
 if(k==='deleted' && ctx.role==='result' && ctx.method==='ui.deleteInterceptRule') prefix='interceptrule';
 if(k==='id' && ctx.role==='params' && /[Tt]askInterceptRule/.test(ctx.method)) prefix='task';
 if(k==='id' && ctx.role==='params' && ctx.method==='ui.taskArchive') prefix='archive';
 if(k==='id' && ctx.role==='params' && ctx.method==='ui.chatUpload') {
  wireOverrides.push({method:ctx.method,field:key,prefix:'task|conversation',role:ctx.role});
  return {anyOf:[id('task'),id('conversation')]};
 }
 if(k==='category_id' && ctx.role==='params') return nullable(id('category'));
 if(k==='active_llm_profile_id' && /updateTaskLLMProfiles/.test(ctx.method)) return nullable(id('profile'));
 if(k==='prompt' && ctx.role==='result' && /getAgent/.test(ctx.method) && s.type==='string') return ref('Body');
 if(k==='goal_id' || k==='intent_id') prefix='node';
 if(k==='before' && /conversationHistory|activityHistory|intentsPage/.test(ctx.method)) return nullable(id(/conversationHistory/.test(ctx.method)?'conversationactivity':/activityHistory/.test(ctx.method)?'activity':'node'));
 if(k==='before') prefix=/conversationHistory/.test(ctx.method)?'conversationactivity':/activityHistory/.test(ctx.method)?'activity':/intentsPage/.test(ctx.method)?'node':null;
 if(k==='since' && /conversationMessages/.test(ctx.method)) prefix='conversationactivity';
 if(k==='cursor' && /conversationMessages|conversationHistory/.test(ctx.method)) return nullable(id('conversationactivity'));
 if(['cursor','snapshot_cursor','earliest_cursor'].includes(k) && /activity|activityHistory/.test(ctx.method)) return nullable(id('activity'));
 if(k==='since' && ctx.method==='ui.activity') return nullable(id('activity'));
 if(['seq','current'].includes(k) && /mainSessions|newMainSession/.test(ctx.method)) return {type:'integer',minimum:0,maximum:2147483647};
 if(k==='task_id' && typeName==='SkillCall') return nullable(id('task'));
 if(k==='session_id' && typeName==='SkillCall') return str;
 if(ctx.role==='result' && ctx.method==='ui.interceptExecution' && typeName==='Activity' && k==='seq') {
  wireOverrides.push({method:ctx.method,field:key,prefix:'activity|conversationactivity',role:ctx.role});
  return {anyOf:[id('activity'),id('conversationactivity')]};
 }
 if(k==='seq' && (typeName==='Activity' || /conversationMsgDetail/.test(ctx.method))) prefix=/[Cc]onversation/.test(ctx.method)?'conversationactivity':'activity';
 if(k==='seq' && typeName==='InterceptExecution') return {anyOf:[id('activity'),id('conversationactivity')]};
 if(k==='before' && /log/i.test(ctx.method) && s.type==='number') return uint;
 if(prefix) {
  const replacement=s.type==='array'||k==='ids'?array(id(prefix)):id(prefix);
  wireOverrides.push({method:ctx.method,field:key,prefix,role:ctx.role});
  return preserveNull(s,replacement);
 }
 if(k==='seq' && s.type==='number') return uint;
 if(pluralIds[k] && (s.type==='array'||s.anyOf?.some(x=>x.type==='array'))) return preserveNull(s,array(id(pluralIds[k])));
 if(['version','content_version','evidence_version','report_evidence_version','generation','llm_chain_revision','round_no','size_bytes','original_size','compressed_size'].includes(k) && (s.type==='number'||s.anyOf?.some(x=>x.type==='number'))) return preserveNull(s,uint);
 if((typeName==='TrafficExchange' && ['req_len','resp_len'].includes(k))||typeName==='TrafficEvidenceSnapshot' && ['captured_at','req_len','resp_len'].includes(k)) return preserveNull(s,uint);
 const result={...s};
 if(s.properties) result.properties=Object.fromEntries(Object.entries(s.properties).map(([name,value])=>[name,wire(value,{...ctx,prefix:childIds[snake(name)]||ctx.prefix},name,typeName)]));
 if(ctx.role==='result'&&typeName==='Activity'&&result.properties) result.properties.stream=obj({run_id:id('run'),turn_id:str,pending:bool});
 if(ctx.role==='result'&&typeName==='Task'&&result.properties) Object.assign(result.properties,{accounting_state:{enum:['known','recovery_required']},active_elapsed_ms:uint,remaining_timeout_ms:nullable(uint)});
 if(ctx.role==='result'&&ctx.method==='ui.createCompany'&&result.properties?.scope_added) result.properties.scope_skipped={type:'integer',minimum:0};
 if(s.items) result.items=wire(s.items,ctx,'',typeName);
 if(s.anyOf) result.anyOf=s.anyOf.map(x=>wire(x,ctx,key,typeName));
 if(s.additionalProperties && typeof s.additionalProperties==='object') result.additionalProperties=wire(s.additionalProperties,ctx,'',typeName);
 return result;
}
for (const op of ui.operations) {
  const ctx={method:op.method,prefix:operationPrefix(op.method),role:'params'};
  const properties = Object.fromEntries(op.parameters.map(p => [p.name, wire(p.schema,ctx,p.name)]));
  methods[op.method] = obj(properties, op.parameters.filter(p => !p.optional).map(p => p.name));
}
methods['ui.recoverTaskBudget']=obj({id:id('task'),mode:{enum:['use_recorded','override']},elapsedActiveMs:nullable(uint)});
methods['ui.chatUpload'].allOf=[
 {if:{properties:{scope:{const:'task'}},required:['scope']},then:{properties:{id:id('task')}}},
 {if:{properties:{scope:{const:'session'}},required:['scope']},then:{properties:{id:id('conversation')}}}
];
const routeRegistry = read('legacy-routes.v1.json');
for (const r of routeRegistry.routes) if (r.disposition !== 'removed_os_session') {
  methods[r.rpc_method] ??= obj({ route_id:{ enum:[] }, path:{ type:'object' }, query:{ type:'object' }, body:{}, blob_ref:nullable(ref('BlobRef')) }, ['route_id','path','query','body']);
  methods[r.rpc_method].properties.route_id.enum.push(r.route_id);
}
const workspaceResult = obj({ workspace_id:id('workspace'), name:str, format_version:{ const:1 }, commit_seq:uint, read_only:bool, recovery:{ type:'object' }, capabilities:array(str) });
const executionResult = obj({ execution_id:id('exec'),state:{ enum:['prepared','running','completed','failed','cancelled','unknown'] },exit_code:nullable({ type:'integer' }),output_ref:nullable(ref('BlobRef')),last_cursor:uint },['execution_id','state']);
const results = {
 'system.hello':obj({ protocol:{ const:'artex-rpc/1.0' }, engine_epoch:id('epoch'),engine_version:str,store_format:{ const:1 },max_frame_bytes:{ const:1048576 },max_blob_chunk:{ const:65536 },read_only:bool,workspace_id:nullable(id('workspace')),commit_seq:uint,capabilities:array(str) }),
 'system.ping':obj({engine_epoch:id('epoch'),commit_seq:uint,uptime_ms:uint}),'system.shutdown':obj({accepted:bool}),
 'workspace.create':workspaceResult,'workspace.open':workspaceResult,'workspace.info':workspaceResult,
 'workspace.close':obj({closed:bool}),'workspace.checkpoint':obj({generation:uint,base_seq:uint,manifest_sha256:digest}),
 'workspace.recover':obj({mode:{ enum:['normal','read_only','recovery_required'] },valid_generations:array(uint),last_valid_seq:uint,corruption:nullable({type:'object'}),lost_range:nullable(obj({from:uint,to:uint})),recovery_copy_capability:nullable(str)}),
 'query.objects':obj({items:array(obj({header:ref('EntityHeader'),data:{}})),read_seq:uint,next_cursor:nullable(str)}),
 'command.status':obj({state:{enum:['unknown','pending','committed']},commit_seq:uint,result:{},result_ref:nullable(ref('BlobRef'))},['state']),
 'command.prepare':obj({expected:array(ref('Expected')),read_seq:uint}),
 'rpc.cancel':obj({accepted:bool,committed:bool}),
 'events.subscribe':obj({subscription_id:str,after_seq:uint,high_watermark:uint}), 'events.unsubscribe':obj({removed:bool}),
 'blob.begin':obj({transfer_id:id('transfer'),next_offset:uint}), 'blob.write':obj({next_offset:uint}), 'blob.commit':ref('BlobRef'),
 'blob.read':obj({offset:uint,length:uint,total_length:uint,data_base64:str,eof:bool}),
 'tool.invoke':obj({invocation_id:id('invocation'),state:common.ToolInvocation.properties.state,result:ref('ToolResult'),result_ref:nullable(ref('BlobRef'))},['invocation_id','state']),
 'agent.send':obj({message_id:id('message'),delivery_ids:array(id('delivery')),request_id:nullable(id('message'))}),
 'agent.inbox':obj({items:array(obj({message:agentMessage,delivery:ref('AgentDelivery')})),next_cursor:nullable(str),read_seq:uint}),
 'agent.claim':obj({claimed:bool,owner:nullable(ref('Actor')),claim_deadline:nullable(timestamp)}),
 'agent.acknowledge':obj({state:common.AgentDelivery.properties.state}), 'agent.cancel_request':obj({state:request.properties.state}),
 'user.answer':obj({accepted:bool,state:common.UserQuestion.properties.state}), 'host.secret.set':ref('SecretRef'),'host.secret.delete':obj({deleted:bool}),
 'agent.features.get':ref('AgentFeatures'),'agent.features.set':ref('AgentFeatures'),
 'host.execute':executionResult,'host.execution.status':executionResult,'host.execution.stop':obj({accepted:bool,state:executionResult.properties.state}),
 'host.update.check':obj({configured:bool,available:bool,package_id:str,version:str},['configured','available']),'host.update.apply':obj({accepted:bool})
};
for (const op of ui.operations) results[op.method] = wire(op.result_schema,{method:op.method,prefix:operationPrefix(op.method),role:'result'});
for(const method of ['task.recover_budget','ui.recoverTaskBudget']) results[method]=obj({task_id:id('task'),accounting_state:{const:'known'},active_elapsed_ms:uint,remaining_timeout_ms:nullable(uint)});
// The Go chat endpoint has both immediate responses and asynchronous LLM admission.
results['ui.chat'] = obj({reply:str,status:str,mode:str},['mode']);
for (const method of ['ui.conversationMsgDetail','ui.activityDetail']) if(results[method]?.type==='string') results[method]=ref('Body');
save('ui-wire-overrides.v1.json',{version:'1.0',identity_fields:wireOverrides,source:'ui-dtos.v1.json',wire_definitions:'rpc.v1.schema.json',rules:'desktop-implementation-spec.md#원본-api의-전환과-dto-변환'});
const errorCodes = { INVALID_REQUEST:-32600,INVALID_PARAMS:-32602,METHOD_NOT_FOUND:-32601,PARSE_ERROR:-32700 };
for (const c of ['VERSION_MISMATCH','WORKSPACE_NOT_OPEN','STALE_EPOCH','NOT_FOUND','VERSION_CONFLICT','EVIDENCE_CONFLICT','COMMAND_ID_REUSE','PERMISSION_DENIED','FEATURE_DISABLED','BUSY','CANCELLED','TIMEOUT','SIZE_LIMIT','IO_FAILURE','CORRUPT_STORE','RECOVERY_REQUIRED','RESYNC_REQUIRED','EXTERNAL_FAILURE','REQUEST_CYCLE','ALREADY_COMPLETED','CURSOR_EXPIRED']) errorCodes[c] = -32001-Object.keys(errorCodes).length+4;
const rpcContract = { $schema:dialect, $id:'urn:artex:rpc:1', $defs:{ ...common, ...wireDefs,
 Request:obj({ jsonrpc:{ const:'2.0' }, protocol:{ const:'artex-rpc/1.0' }, id:str, method:{ enum:Object.keys(methods) }, workspace_id:nullable(id('workspace')), engine_epoch:nullable(id('epoch')), command_id:nullable(id('command')), expected:array(ref('Expected')), timeout_ms:{ type:'integer', minimum:1, maximum:3600000 }, params:{} }),
 Success:obj({ jsonrpc:{ const:'2.0' }, id:str, engine_epoch:id('epoch'), read_seq:uint, result:{}, read_versions:array(ref('Expected')) },['jsonrpc','id','engine_epoch','read_seq','result']),
 Failure:obj({ jsonrpc:{ const:'2.0' }, id:nullable(str), engine_epoch:id('epoch'), error:ref('Error') }),
 Event:obj({ jsonrpc:{ const:'2.0' }, method:{ const:'event' }, params:obj({ subscription_id:str, engine_epoch:id('epoch'), seq:uint, commit_seq:uint, kind:str, payload:{} }) })
 }, methods, results, error_codes:errorCodes, legacy_routes:'legacy-routes.v1.json', ui_contracts:'ui-dtos.v1.json', result_rules:'desktop-implementation-spec.md#rpc-메서드와-결과' };
extendWorkspaceApprovalSettings(rpcContract);
save('rpc.v1.schema.json', rpcContract);
const manifestProperties = { format:{const:'artex-checkpoint'},format_version:{const:1},workspace_id:id('workspace'),generation:uint,base_seq:uint,base_frame_sha256:digest,created_at:timestamp,files:array(obj({path:str,sha256:digest,length:uint,record_count:uint})),journal_start_seq:uint,command_receipt_count:uint,manifest_sha256:digest };
save('storage.v1.schema.json', { $schema:dialect,$id:'urn:artex:storage:1',$defs:{ ...definitions,
 Project:obj({format:{const:'artex-workspace'},format_version:{const:1},workspace_id:id('workspace'),name:str,created_at:timestamp}),
 Manifest:obj(manifestProperties),
 Mutation:obj({op:{enum:['put','delete']},collection:{enum:allCollections},id:str,expected_version:uint,new_version:uint,record:ref('RecordEnvelope')},['op','collection','id','expected_version','new_version']),
 JournalPayload:obj({format_version:{const:1},committed_at:timestamp,command_id:id('command'),method:str,normalized_params:{},expected:array(ref('Expected')),mutations:array(ref('Mutation')),events:array(ref('RecordEnvelope')),deliveries:array(ref('RecordEnvelope')),result:{}})
 } });
const descriptions = {
 add_company_scope:'회사와 회사별 자산 범위를 등록·보강', add_hint:'현재 작업에 계획 지침과 증거 연결 추가', add_intent:'실행할 의도와 부모·자산 관계 생성', add_task_hint:'지정 작업에 지침 전달', add_task_scope:'현재 작업의 실행 대상 범위 추가', bind_finding_traffic:'취약점에 트래픽 증거를 복사·연결', create_custom_tool:'명령·스크립트·HTTP 사용자 도구 등록', create_mcp:'MCP 서버 설정 등록', create_skill:'Skill 지침 파일 생성', delete_assets_by_host:'호스트에 속한 자산 삭제', expand_digest:'압축 요약의 원본 구성 노드 조회', get_finding_retest_context:'현재 재검증의 취약점·증거 문맥 조회', get_finding_traffic:'취약점의 증거 목록과 본문 구간 조회', get_task_graph:'지정 작업의 탐색 그래프 조회', get_task_node_detail:'지정 작업의 노드 상세 조회', get_task_worker_trace:'지정 작업의 의도 실행 기록 조회', get_worker_output:'의도의 실행 결과 조회', get_worker_trace:'의도별 실행 기록 필터 조회', goal_met:'현재 작업의 전체 목표 달성 선언', graph_overview:'현재·직접 원본 작업의 그래프 요약 조회', insert_assets:'유형별 자산 병합 등록과 출처 연결', kill_work:'실행 의도 중단', list_assets:'자산 ID·유형·DSL 조건 검색', list_companies:'회사 이름 검색', list_facts:'현재·직접 원본 작업의 사실 검색', list_findings:'현재·직접 원본 작업의 취약점 조회', list_goals:'현재 목표와 달성 상태 조회', list_llm_profiles:'사용 가능한 모델 프로필 조회', list_task_findings:'지정 작업의 취약점 조회', list_task_worker_traces:'지정 작업의 실행 기록 목록 조회', list_tasks:'전체 작업 목록 조회', list_untested_assets:'현재 범위의 미검사 자산 조회', node_detail:'허용된 노드와 연결·상세 조회', pause_task:'지정 작업을 일시 정지', prove_goal:'사실·취약점 증거로 목표 달성 입증', record_fact:'의도 결과를 사실·자산 관계로 저장', record_finding_retest_result:'재현·수정·불확정 판정과 증거 저장', report_finding:'취약점 노드·레코드·증거를 함께 등록', search_all_worker_traces:'허용된 실행 기록에서 문자열 검색', search_task_worker_traces:'지정 작업의 실행 기록 검색', set_constraints:'허용·금지 제약 추가', set_goals:'목표·취약점 분류 추가', spawn_task:'독립 작업·직접 원본 관계 생성 후 실행 예약', steer_work:'현재 실행의 다음 도구 호출 전 수정 지시', traffic_blob:'해시 본문의 구간 읽기', traffic_get:'트래픽 요청·응답 상세 조회', traffic_search:'호스트·문자열·본문 조건으로 트래픽 검색', update_custom_tool:'사용자 도구 정의와 바인딩 수정', update_finding_report:'발견 노드에 대응하는 취약점 보고서 수정', update_mcp:'MCP 설정 수정', update_skill_file:'Skill 상대 경로 파일 수정', AskUserQuestion:'사용자에게 선택 질문을 제시하고 응답 대기', Bash:'단일 셸 명령 실행·백그라운드 실행', Compress:'원본 대화를 보관하고 범위를 요약으로 교체', Edit:'파일의 일치 문자열 교체', EnterPlanMode:'에이전트를 계획 권한 모드로 전환', ExecuteExtraTool:'검색으로 해제한 추가 도구 실행', ExitPlanMode:'계획을 제시하고 사용자 승인 후 권한 모드 복원', Glob:'경로에서 파일 이름 패턴 검색', Grep:'파일 내용을 정규식으로 검색', LS:'디렉터리 항목 조회', Monitor:'명령 출력을 감시하고 조건 충족 알림', MultiEdit:'파일에 여러 문자열 교체를 순서대로 적용', Read:'파일의 행 구간 읽기', RecallMemory:'에이전트의 장기 기억 검색', RecordMemory:'분류된 장기 기억 저장', SearchExtraTools:'추가 도구 검색과 실행 스키마 공개', Skill:'허용된 Skill 지침 로드', TaskList:'현재 실행의 백그라운드 명령 목록 조회', TaskOutput:'백그라운드 명령 출력 조회·완료 대기', TaskStop:'백그라운드 명령 중단', TodoWrite:'현재 실행의 할 일 목록 교체', WebFetch:'웹 문서를 요청하고 텍스트 추출', Write:'파일 내용 저장', shell_close:'PTY 세션과 프로세스 종료', shell_list:'현재 실행의 PTY 세션 목록 조회', shell_open:'대화형 PTY 세션 시작', shell_read:'PTY 출력 커서·화면 읽기', shell_send:'PTY에 텍스트·키·바이트 전송', sleep:'취소 가능한 시간 대기', web_search:'설정된 검색 백엔드로 웹 검색'
};
const writes = new Set('add_company_scope add_hint add_intent add_task_hint add_task_scope bind_finding_traffic create_custom_tool create_mcp create_skill delete_assets_by_host goal_met insert_assets kill_work pause_task prove_goal record_fact record_finding_retest_result report_finding set_constraints set_goals spawn_task steer_work update_custom_tool update_finding_report update_mcp update_skill_file'.split(' '));
const newTools = new Set(['AskUserQuestion','EnterPlanMode','ExitPlanMode','RecallMemory','RecordMemory']);
const sourceFiles = read('source-manifest.v1.json').files.filter(x => /^(agent|server|traffic|norma\/(tool|plan|memory|noa|skill))\//.test(x.path) && x.path.endsWith('.go')).map(x => ({ path:x.path, text:fs.readFileSync(path.join(originalRoot,x.path),'utf8') }));
function sourceFor(name) {
  const escaped = name.replace(/[.*+?^${}()|[\]\\]/g,'\\$&');
  const r = new RegExp(`(?:Name:\\s*|(?:readTool|writeTool|readExpTool|writeExpTool|sessTool|roTool|wrTool)\\(\\s*)"${escaped}"`);
  const explicit = { Compress:'norma/noa/compress.go', EnterPlanMode:'norma/plan/plan.go', ExitPlanMode:'norma/plan/plan.go', RecallMemory:'norma/memory/memory.go', RecordMemory:'norma/memory/memory.go', Skill:'norma/skill/skill.go', SearchExtraTools:'norma/tool/deferred.go', ExecuteExtraTool:'norma/tool/deferred.go' };
  let f = sourceFiles.find(x => r.test(x.text));
  if (!f && explicit[name]) f = sourceFiles.find(x => x.path === explicit[name]);
  if (!f) f = sourceFiles.find(x => x.text.includes(`"${name}"`));
  if (!f) throw Error('No source for ' + name);
  const at = r.exec(f.text)?.index ?? f.text.indexOf(name);
  return { file:f.path, line:f.text.slice(0,Math.max(at,0)).split('\n').length };
}
function moduleFor(t) {
  const n = t.name;
  if (t.family === 'sdk') return ['Read','Write','Edit','MultiEdit','LS','Glob','Grep'].includes(n) ? 'tools::files' : n.startsWith('shell_') ? 'tools::pty' : ['Bash','Monitor','TaskList','TaskOutput','TaskStop'].includes(n) ? 'tools::process' : ['EnterPlanMode','ExitPlanMode','AskUserQuestion'].includes(n) ? 'tools::interaction' : ['RecallMemory','RecordMemory'].includes(n) ? 'tools::memory' : ['SearchExtraTools','ExecuteExtraTool'].includes(n) ? 'tools::deferred' : ['WebFetch','web_search'].includes(n) ? 'tools::web' : n === 'Compress' ? 'engine::context' : n === 'Skill' ? 'tools::skills' : n === 'sleep' ? 'tools::timer' : 'tools::todos';
  if (n.startsWith('traffic_')) return 'domain::traffic';
  if (n.includes('retest')) return 'domain::retests';
  if (n.includes('finding')) return 'domain::findings';
  if (n.includes('custom_tool') || n.includes('mcp') || n.includes('skill')) return 'domain::configuration';
  if (n.includes('asset') || n.includes('company') || n.includes('scope')) return 'domain::assets';
  if (n.includes('worker') || ['kill_work','steer_work'].includes(n)) return 'engine::workers';
  if (n.includes('task') || n === 'list_llm_profiles') return 'domain::tasks';
  return 'domain::exploration';
}
const specialId = { update_finding_report:'node', node_detail:'node', get_task_node_detail:'node', expand_digest:'node', list_assets:'asset', traffic_get:'traffic', update_mcp:'mcp' };
function overlay(s, t, key = '') {
  if (!s || typeof s !== 'object') return s;
  let prefix = foreign[key] || { intent_id:'node', goal_id:'node', evidence_id:'node', evidence_hint_id:'node', binding_id:'binding', traffic_id:'traffic' }[key];
  if (key === 'finding_id' && t.name === 'update_finding_report') prefix = 'node';
  if (key === 'id') prefix = specialId[t.name];
  if (key === 'task_id' && t.family === 'sdk') prefix = 'exec';
  if (key === 'session_id' && t.name.startsWith('shell_')) prefix = 'shell';
  if (prefix) return { ...id(prefix), description:s.description, 'x-legacy-type':s.type };
  if (['evidence_version'].includes(key)) return { ...uint, description:s.description };
  if (key === 'before' && t.name === 'list_facts') return {...id('node'),description:s.description};
  if (key === 'offset' && ['traffic_blob','get_finding_traffic'].includes(t.name)) return {...uint,description:s.description};
  if (key === 'step_ids') return {...s,items:id('activity')};
  if (['ids','asset_ids','parent_ids','source_task_ids'].includes(key) && s.type === 'array') return { ...s, items:id(key === 'asset_ids' || key === 'ids' && t.name === 'list_assets' ? 'asset' : key === 'source_task_ids' ? 'task' : 'node') };
  const out = { ...s };
  if (s.properties) out.properties = Object.fromEntries(Object.entries(s.properties).map(([k,v]) => [k,overlay(v,t,k)]));
  if (s.items) out.items = overlay(s.items,t,'');
  if (s.anyOf) out.anyOf = s.anyOf.map(v => overlay(v,t,key));
  return out;
}
const tools = read('tools-source.v1.json').map(t => {
  const scope = t.family === 'sdk' ? 'run' : t.name.startsWith('traffic_') ? 'workspace_traffic' : t.default_agents.length ? 'current_task_and_direct_sources_read_only' : 'platform_or_bound_conversation';
  const mutation = t.family === 'artex' ? writes.has(t.name) ? 'workspace_command' : 'read' : t.name === 'RecordMemory' ? 'workspace_command' : ['Read','LS','Glob','Grep','RecallMemory','Skill','TaskList','shell_list'].includes(t.name) ? 'read' : ['Write','Edit','MultiEdit','Bash','Monitor','TaskStop','shell_open','shell_send','shell_close','WebFetch','web_search'].includes(t.name) ? 'external_effect' : 'run_state';
  const desktopSchema = overlay(t.input_schema,t);
  if (t.name === 'update_finding_report') { desktopSchema.properties.finding_node_id = desktopSchema.properties.finding_id; delete desktopSchema.properties.finding_id; desktopSchema.required = desktopSchema.required.map(x => x === 'finding_id' ? 'finding_node_id' : x); if (!desktopSchema.required.includes('evidence_version')) desktopSchema.required.push('evidence_version'); }
  if (t.name === 'AskUserQuestion') Object.assign(desktopSchema.properties.questions,{minItems:1,maxItems:4});
  if (t.name === 'RecordMemory') Object.assign(desktopSchema.properties.type,{enum:['user','feedback','project','reference'],default:'project'});
  const seedAgents = [...t.default_agents];
  if (t.family === 'artex' && !seedAgents.length && t.name !== 'goal_met' && !t.name.startsWith('traffic_') && !t.name.includes('retest')) seedAgents.push('auto');
  if (['update_finding_report','get_task_node_detail','list_task_findings','get_task_worker_trace','list_task_worker_traces','search_task_worker_traces','get_task_graph'].includes(t.name)) seedAgents.push('reporter');
  if (t.name.startsWith('traffic_')) seedAgents.push('worker','reporter');
  if (t.name === 'get_finding_traffic') seedAgents.push('reporter','worker','planner','mainagent','pentest');
  if (t.name === 'bind_finding_traffic') seedAgents.splice(0,seedAgents.length,'auto','reporter');
  if (t.name.includes('retest')) seedAgents.push('retester');
  const handler = t.name.replace(/([a-z0-9])([A-Z])/g,'$1_$2').toLowerCase();
  return { ...t, description_ko:descriptions[t.name], source:sourceFor(t.name), rust_handler:`${moduleFor(t)}::${handler}`, fresh_seed_agents:[...new Set(seedAgents)], scope, mutation, desktop_status:newTools.has(t.name) ? 'implement_opt_in_default_off' : t.name === 'goal_met' ? 'implement_unbound_default' : 'implement_with_existing_gates', desktop_input_schema:desktopSchema, input_mode:t.name === 'Compress' ? 'lenient_raw_json' : 'json_object', compatibility_rule:t.name === 'update_finding_report' ? 'v0 finding_id is NodeId; v1 finding_node_id and required evidence_version' : 'preserve non-ID parameters, defaults, partial-success and tool_result pairing; replace numeric IDs with typed strings' };
});
if (tools.length !== 81 || tools.some(t => !t.description_ko)) throw Error('Incomplete tool matrix');
save('tools.v1.json', { contract_version:'1.0', source_definition_count:81, artex_count:51, sdk_count:30, tools });
const rows = tools.map((t,i) => `| ${i+1} | \`${t.name}\` | ${t.description_ko} | \`${t.rust_handler}\` | ${t.fresh_seed_agents.join(', ') || (t.desktop_status.includes('default_off') ? '선택 활성화' : '별도 조립·정책')} | ${t.mutation} | [${t.source.file}:${t.source.line}](${path.relative(path.join(root,'docs'),path.join(originalRoot,t.source.file)).split(path.sep).join('/')}#L${t.source.line}) |`);
fs.writeFileSync(path.join(root,'docs/desktop-tool-mapping.md'), `# 데스크톱 도구 대응 명세\n\n2026-10-12. 정의는 ARTEX 51개 + SDK 30개 = **81개**다. 사용자 도구·MCP에서 추가 발견한 도구와 새 내부 메시지 RPC는 이 숫자에 포함하지 않는다. 모든 81개 핸들러를 구현 대상으로 확정한다. 현재 미연결 SDK 5개는 선택 기능으로 구현하고 기본 비활성화한다. 실제 제공 목록은 실행 문맥·에이전트 바인딩·기능 설정·권한 검사를 모두 통과한 교집합이다.\n\n[상세 구현 명세](desktop-implementation-spec.md)의 공통 결과·권한·저장·중복·취소 규칙을 적용한다. 전체 원본 입력 스키마와 v1 스키마, 기본 도메인 바인딩, 소스 위치, 구현 모듈은 [tools.v1.json](contracts/tools.v1.json)에 있다. 빈 입력으로 조사한 read_only/concurrent 값은 참고 자료이며 실제 호출의 권한 또는 병렬성 판정으로 사용하지 않는다.\n\n| 번호 | 이름 | 기능 | Rust 핸들러 | 원본 기본 도메인 바인딩/제공 경로 | 효과 | Go 근거 |\n| --- | --- | --- | --- | --- | --- | --- |\n${rows.join('\n')}\n\n\`workspace_command\`는 객체·관계·이벤트를 같은 저장 명령으로 확정한다. \`external_effect\`는 외부 실행 ID와 승인·완료 기록을 사용한다. \`run_state\`는 문맥·대기·권한 모드를 변경하며 읽기 전용 도구처럼 병렬 실행하지 않는다. SDK 기본 도구는 별도 실행 조립 조건을 따른다. 예를 들어 worker는 기본 MultiEdit·Glob·Grep를 제외하며, PTY·검색·트래픽은 해당 기능 활성화가 필요하다. 플랫폼·재검증 도구의 빈 바인딩은 미구현을 뜻하지 않는다.\n`, 'utf8');
// A byte-for-byte journal frame vector; no runtime implementation is implied.
const payload = Buffer.from(JSON.stringify({ format_version:1, committed_at:'2026-10-12T00:00:00Z', command_id:'command_'+'1'.repeat(32), method:'fixture.noop', normalized_params:{}, expected:[], mutations:[], events:[], deliveries:[], result:{ ok:true } }));
const header = Buffer.alloc(104); header.write('ARTEXJ01',0,'ascii'); header.writeUInt16LE(1,8); header.writeUInt32LE(payload.length,12); header.writeBigUInt64LE(1n,16); Buffer.from('2'.repeat(32),'hex').copy(header,24); crypto.createHash('sha256').update(payload).digest().copy(header,72);
const frame = Buffer.concat([header,payload,Buffer.from('ARTEXEND','ascii')]);
save('journal-vector.v1.json',{ version:1, header_bytes:104, payload_length:payload.length, frame_length:frame.length, seq:'1', previous_frame_sha256:'0'.repeat(64), frame_sha256:crypto.createHash('sha256').update(frame).digest('hex'), frame_hex:frame.toString('hex'), payload:JSON.parse(payload), recovery_cases:[{name:'complete',expected:'replay_once'},{name:'truncate_last_byte',expected:'uncommitted_tail'},{name:'flip_payload_byte',expected:'read_only_corruption'},{name:'duplicate_command_same_params',expected:'return_receipt'},{name:'duplicate_command_changed_params',expected:'COMMAND_ID_REUSE'}] });
const exampleId = p => p+'_'+'1'.repeat(32), now = '2026-10-12T00:00:00Z';
const message = {protocol:'artex-agent/1.0',message_id:exampleId('message'),command_id:exampleId('command'),workspace_id:exampleId('workspace'),kind:'request',sender:{kind:'agent',agent_id:exampleId('agent'),run_id:exampleId('run'),engine_epoch:exampleId('epoch')},recipients:['agent_'+'2'.repeat(32)],channel:'workspace/'+exampleId('workspace')+'/agents',topic:'verify-fact',correlation_id:null,created_at:now,deadline_at:'2026-10-12T00:02:00Z',body:{text:'추가 검증 결과를 알려주세요.',blob:null},object_refs:[{collection:'exploration_nodes',id:exampleId('node'),version:'1',access:'read'}],final:false,outcome:'none',hop_count:0};
save('examples.v1.json',{
 project:{format:'artex-workspace',format_version:1,workspace_id:exampleId('workspace'),name:'ARTEX',created_at:now},
 message,
 rpc_request:{jsonrpc:'2.0',protocol:'artex-rpc/1.0',id:'host-1',method:'agent.send',workspace_id:exampleId('workspace'),engine_epoch:exampleId('epoch'),command_id:exampleId('command'),expected:[],timeout_ms:30000,params:{message,request_options:{completion_mode:'one',expected_reports:message.recipients}}},
 rpc_success:{jsonrpc:'2.0',id:'host-1',engine_epoch:exampleId('epoch'),read_seq:'1',result:{message_id:message.message_id,delivery_ids:[exampleId('delivery')],request_id:message.message_id}},
 rpc_failure:{jsonrpc:'2.0',id:'host-1',engine_epoch:exampleId('epoch'),error:{code:errorCodes.VERSION_CONFLICT,message:'객체가 변경되었습니다.',data:{code:'VERSION_CONFLICT',retryable:false,recovery_required:false,details:{id:exampleId('node'),expected:'1',actual:'2'}}}},
 company:{header:{id:exampleId('company'),collection:'companies',version:'1',commit_seq:'1',created_at:now,updated_at:now,legacy_key:'42'},data:{id:exampleId('company'),name:'Example',nkey:'example',logo:null,created_at:now,updated_at:now}},
 report_input:{finding_node_id:exampleId('node'),report:'재현과 증거를 확인한 보고서입니다.',evidence_version:'1'}
});
console.log(`Defined ${Object.keys(definitions).length} entity schemas, ${Object.keys(methods).length} RPC methods and ${tools.length} tool mappings.`);
