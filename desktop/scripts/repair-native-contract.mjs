// Narrow source-contract correction; does not regenerate unrelated legacy DTOs.
import fs from 'node:fs';
import path from 'node:path';
import assert from 'node:assert/strict';
import crypto from 'node:crypto';
const root=path.resolve(import.meta.dirname,'../..');
const id=prefix=>({type:'string',pattern:`^${prefix}_[0-9a-f]{32}$`});
const directories=['docs/contracts','desktop/contracts'];
if(process.argv.includes('--intercept-activity-seq')) {
 const hash=bytes=>crypto.createHash('sha256').update(bytes).digest('hex');
 const files=directories.flatMap(directory=>fs.readdirSync(path.join(root,directory)).filter(name=>name.endsWith('.json')).map(name=>path.join(root,directory,name)));
 const before=new Map(files.map(filename=>[filename,fs.readFileSync(filename)]));
 const edits=[];
 for(const directory of directories) {
  const filename=path.join(root,directory,'rpc.v1.schema.json');
  const contract=JSON.parse(before.get(filename));
  const original=structuredClone(contract);
  const parentRef=contract.results['ui.interceptExecution'].$ref;
  assert.match(parentRef,/^#\/\$defs\/W\d+$/);
  const parent=contract.$defs[parentRef.split('/').at(-1)];
  const activityRef=parent.properties.items.items.$ref;
  assert.match(activityRef,/^#\/\$defs\/W\d+$/);
  const name=activityRef.split('/').at(-1);
  const activity=contract.$defs[name];
  assert.equal(activity['x-source-type'],'Activity');
  const dual={anyOf:[id('activity'),id('conversationactivity')]};
  assert.deepEqual(parent.properties.seq,dual,'Parent sequence already supports both namespaces');
  assert.ok(JSON.stringify(activity.properties.seq)===JSON.stringify(id('activity'))||JSON.stringify(activity.properties.seq)===JSON.stringify(dual),'Unexpected prior nested sequence schema');
  activity.properties.seq=dual;
  const masked=structuredClone(contract);
  masked.$defs[name].properties.seq=original.$defs[name].properties.seq;
  assert.deepEqual(masked,original,'Only nested interception Activity.seq may change');
  edits.push({filename,document:contract,allowed_paths:[`/$defs/${name}/properties/seq`]});
  const overridesFile=path.join(root,directory,'ui-wire-overrides.v1.json');
  const overrides=JSON.parse(before.get(overridesFile));
  const oldOverrides=structuredClone(overrides);
  const indices=overrides.identity_fields.map((field,index)=>({field,index})).filter(({field})=>field.method==='ui.interceptExecution'&&field.role==='result'&&field.field==='seq');
  assert.equal(indices.length,1,'Expected one nested Activity sequence identity override');
  for(const {field} of indices) {
   assert.ok(['activity','activity|conversationactivity'].includes(field.prefix));
   field.prefix='activity|conversationactivity';
  }
  const maskedOverrides=structuredClone(overrides);
  for(const {index} of indices)maskedOverrides.identity_fields[index].prefix=oldOverrides.identity_fields[index].prefix;
  assert.deepEqual(maskedOverrides,oldOverrides,'Only exact interception sequence identity metadata may change');
  edits.push({filename:overridesFile,document:overrides,allowed_paths:indices.map(({index})=>`/identity_fields/${index}/prefix`)});
 }
 // Validate the complete change plan before publishing any edited contract.
 const changes=[];
 for(const edit of edits) {
  const bytes=Buffer.from(JSON.stringify(edit.document,null,2)+'\n');
  if(!bytes.equals(before.get(edit.filename))) {
   fs.writeFileSync(edit.filename,bytes);
   changes.push({file:path.relative(root,edit.filename).replaceAll('\\','/'),before_sha256:hash(before.get(edit.filename)),after_sha256:hash(bytes),allowed_paths:edit.allowed_paths});
  }
 }
 const allowed=new Set(edits.map(edit=>edit.filename));
 for(const filename of files)if(hash(fs.readFileSync(filename))!==hash(before.get(filename)))assert.ok(allowed.has(filename),'Unrelated contract changed');
 for(const directory of directories) {
  const filename=path.join(root,directory,'ui-dtos.v1.json');
  assert.ok(fs.readFileSync(filename).equals(before.get(filename)),'Frozen legacy UI DTOs must remain byte-identical');
 }
 console.log(JSON.stringify({mode:'intercept-activity-seq',changed_contracts:changes,all_other_contracts_byte_identical:true},null,2));
} else for(const directory of directories){
 const filename=path.join(root,directory,'rpc.v1.schema.json');
 const contract=JSON.parse(fs.readFileSync(filename,'utf8'));
 const upload=contract.methods['ui.chatUpload'];
 contract.methods['ui.taskArchive'].properties.id=id('archive');
 upload.properties.id={anyOf:[id('task'),id('conversation')]};
 upload.allOf=[
  {if:{properties:{scope:{const:'task'}},required:['scope']},then:{properties:{id:id('task')}}},
  {if:{properties:{scope:{const:'session'}},required:['scope']},then:{properties:{id:id('conversation')}}}
 ];
 fs.writeFileSync(filename,JSON.stringify(contract,null,2)+'\n');
 const overridesFile=path.join(root,directory,'ui-wire-overrides.v1.json');
 const overrides=JSON.parse(fs.readFileSync(overridesFile,'utf8'));
 for(const field of overrides.identity_fields)if(field.method==='ui.chatUpload'&&field.role==='params'&&field.field==='id')field.prefix='task|conversation';
 for(const field of overrides.identity_fields)if(field.method==='ui.taskArchive'&&field.role==='params'&&field.field==='id')field.prefix='archive';
 fs.writeFileSync(overridesFile,JSON.stringify(overrides,null,2)+'\n');
}
