import fs from 'node:fs';
import path from 'node:path';
import assert from 'node:assert/strict';
const root=path.resolve(import.meta.dirname,'..');
const coverage=JSON.parse(fs.readFileSync(path.join(root,'crates/tools/tool-coverage.v1.json'),'utf8'));
const definitions=JSON.parse(fs.readFileSync(path.join(root,'contracts/tools.v1.json'),'utf8')).tools;
assert.equal(coverage.tools.length,81);
assert.deepEqual(coverage.tools.map(t=>t.name).sort(),definitions.map(t=>t.name).sort());
const evidence=new Set();
for(const tool of coverage.tools){
  assert.equal(tool.executor_present,true,tool.name);
  assert.ok(tool.actual_executor_path?.length,tool.name);
  assert.ok(tool.verification_evidence?.length,tool.name);
  for(const reference of tool.verification_evidence){
    const parts=reference.split('::'),relative=parts[0],name=parts.at(-1);
    const file=path.resolve(root,'crates',relative);
    assert.ok(file.startsWith(path.join(root,'crates')+path.sep),reference);
    const source=fs.readFileSync(file,'utf8');
    assert.ok(new RegExp('fn\\s+'+name+'\\s*\\(').test(source),reference);
    evidence.add(reference);
  }
}
console.log(`Tool evidence audit PASS: 81 executable mappings, ${evidence.size} named evidence functions. This checks references; behavioral test results are recorded separately.`);
