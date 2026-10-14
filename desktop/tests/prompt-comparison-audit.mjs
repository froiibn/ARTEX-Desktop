// Read-only comparison against the original Git checkout and localized Go source.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import {execFileSync} from 'node:child_process';
import assert from 'node:assert/strict';
import {requireOriginalRoot} from '../../scripts/source-layout.mjs';
const repo=path.resolve(import.meta.dirname,'../..');
const originalRepo=requireOriginalRoot();
const commit=execFileSync('git',['rev-parse','HEAD'],{cwd:originalRepo,encoding:'utf8'}).trim();
function extract(source,name,ancestors=new Set()){
  if(ancestors.has(name))throw Error('Recursive constant '+name);
  ancestors=new Set([...ancestors,name]);
  const match=new RegExp('\\bconst\\s+'+name+'\\s*=\\s*').exec(source);
  if(!match)throw Error('Constant missing: '+name);
  let pos=match.index+match[0].length,value='';
  function whitespace(){while(/\s/.test(source[pos]??''))pos++;}
  while(true){
    whitespace();const start=pos;
    if(source[pos]==='`'){
      pos++;const end=source.indexOf('`',pos);if(end<0)throw Error('Unclosed raw constant');
      value+=source.slice(pos,end).replaceAll('\r','');pos=end+1;
    }else if(source[pos]==='"'){
      pos++;while(pos<source.length){if(source[pos]==='\\'){pos+=2;continue;}if(source[pos++]==='"')break;}
      value+=JSON.parse(source.slice(start,pos));
    }else if(/^[A-Za-z_]\w*/.test(source.slice(pos))){
      const identifier=/^[A-Za-z_]\w*/.exec(source.slice(pos))[0];
      value+=extract(source,identifier,ancestors);pos+=identifier.length;
    }else throw Error('Unsupported constant expression '+name+' at '+source.slice(pos,pos+30));
    whitespace();if(source[pos]!=='+')break;pos++;
  }
  return value;
}
const groups=[
  {kind:'system',file:'desktop/crates/domain/resources/builtin-prompts.v1.json',rows:[
    ['goals','agent/goals.go','goalsDefaultTmpl'],['planner','agent/planner.go','plannerDefaultTmpl'],
    ['mainagent','agent/mainagent.go','mainAgentDefaultTmpl'],['worker','agent/worker.go','workerDefaultTmpl'],
    ['auto','agent/promptcatalog.go','autoDefaultTmpl'],['pentest','agent/promptcatalog.go','pentestDefaultTmpl'],
    ['assistant','agent/promptcatalog.go','DefaultAssistantPrompt'],['reporter','agent/promptcatalog.go','ReporterDefaultPrompt'],
    ['retester','agent/retester.go','RetesterDefaultPrompt']]},
  {kind:'wrapup',file:'desktop/crates/domain/resources/builtin-wrapup.v1.json',rows:[
    ['worker','agent/worker.go','settleWrapUpPrompt'],['planner','agent/wrapup.go','plannerWrapUpDefault'],
    ['mainagent','agent/wrapup.go','mainAgentWrapUpDefault'],['generic','agent/wrapup.go','genericWrapUpDefault'],
    ['worker_timeout','agent/wrapup.go','workerTaskTimeoutDefault'],['planner_timeout','agent/wrapup.go','plannerTaskTimeoutDefault']]},
  {kind:'judge',file:'desktop/crates/domain/src/policy-prompts.json',rows:[
    ['DefaultJudgePrompt','intercept/prompt.go','DefaultJudgePrompt'],['JudgeContextBoundary','intercept/prompt.go','JudgeContextBoundary'],
    ['JudgeOutputContract','intercept/prompt.go','JudgeOutputContract']]},
];
const digest=s=>crypto.createHash('sha256').update(s).digest('hex');
const metrics=s=>({characters:[...s].length,bytes:Buffer.byteLength(s),sha256:digest(s)});
const result={audited_at:new Date().toISOString(),baseline_commit:commit,scope:'Code defaults; no active user workspace or provider request inspected',prompts:[]};
const cache=new Map();
for(const group of groups){
  const catalog=JSON.parse(fs.readFileSync(path.join(repo,group.file),'utf8'));
  for(const [key,file,name]of group.rows){
    if(!cache.has(file))cache.set(file,{original:execFileSync('git',['show',commit+':'+file],{cwd:originalRepo,encoding:'utf8',maxBuffer:4*1024*1024}),localized:fs.readFileSync(path.join(originalRepo,file),'utf8')});
    const pair=cache.get(file),original=extract(pair.original,name),localized=extract(pair.localized,name),current=catalog[key];
    if(typeof current!=='string')throw Error('Current prompt missing '+key);
    let mismatch=0;while(mismatch<Math.min(current.length,localized.length)&&current[mismatch]===localized[mismatch])mismatch++;
    const item={kind:group.kind,key,source:file,constant:name,current_file:group.file,original:metrics(original),localized:metrics(localized),current:metrics(current),current_equals_localized:current===localized,original_equals_localized:original===localized,current_is_truncated_localized_prefix:localized.startsWith(current)&&current.length<localized.length,first_localized_difference:current===localized?null:mismatch,lost_localized_tail:localized.startsWith(current)?localized.slice(current.length):null};
    result.prompts.push(item);
    console.log(JSON.stringify({...item,lost_localized_tail:item.lost_localized_tail?{characters:[...item.lost_localized_tail].length,preview:item.lost_localized_tail.slice(0,170),end:item.lost_localized_tail.slice(-170)}:null}));
  }
}
if(process.argv.includes('--assert')){
  for(const item of result.prompts.filter(item=>item.kind!=='judge'))assert.equal(item.current_equals_localized,true,'Builtin prompt must include the full Go constant expression: '+item.key);
  for(const item of result.prompts.filter(item=>item.kind==='judge'&&item.key!=='DefaultJudgePrompt'))assert.equal(item.current_equals_localized,true,'Judge contract mismatch: '+item.key);
  const judge=result.prompts.find(item=>item.kind==='judge'&&item.key==='DefaultJudgePrompt');
  const policy=JSON.parse(fs.readFileSync(path.join(repo,'desktop/crates/domain/src/policy-prompts.json'),'utf8'));
  assert.equal(judge.current_is_truncated_localized_prefix,true);
  assert.equal(judge.lost_localized_tail,policy.JudgeOutputContract,'Separately appended judge contract must preserve the complete source suffix');
  result.default_catalog_passed=true;
  console.log('Prompt catalog audit PASS: 9 complete role defaults, 6 wrapup defaults, 3 judge sections. Runtime assembly is checked by Rust and actual HTTP request fixtures.');
}
fs.writeFileSync(path.join(import.meta.dirname,'prompt-comparison-audit.json'),JSON.stringify(result,null,2)+'\n');
