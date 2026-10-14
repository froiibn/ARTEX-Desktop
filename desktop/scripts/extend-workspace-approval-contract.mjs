import fs from 'node:fs';
import path from 'node:path';
import assert from 'node:assert/strict';
import crypto from 'node:crypto';
import {extendWorkspaceApprovalSettings} from '../../scripts/native-settings-contract.mjs';
const root = path.resolve(import.meta.dirname, '../..');
const directories = ['docs/contracts', 'desktop/contracts'];
const hash = bytes => crypto.createHash('sha256').update(bytes).digest('hex');
const before = new Map(directories.flatMap(directory => fs.readdirSync(path.join(root,directory))
  .filter(name => name.endsWith('.json')).map(name => {
    const file = path.join(root,directory,name);
    return [file,fs.readFileSync(file)];
  })));
const plans = directories.map(directory => {
  const file = path.join(root,directory,'rpc.v1.schema.json');
  const contract = JSON.parse(before.get(file));
  const original = structuredClone(contract);
  const changed = extendWorkspaceApprovalSettings(contract);
  const masked = structuredClone(contract);
  const maskedSchemas = extendWorkspaceApprovalSettings(masked);
  // Remove the desktop extension from both snapshots before comparison.
  for (const schemas of [maskedSchemas,extendWorkspaceApprovalSettings(original)]) {
    for (const schema of schemas) delete schema.properties.approval_bypass;
  }
  assert.deepEqual(masked,original,'Only the three settings approval_bypass properties may change');
  assert.equal(changed.length,3);
  return {file,bytes:Buffer.from(JSON.stringify(contract,null,2)+'\n')};
});
const changes=[];
for(const {file,bytes} of plans) if(!bytes.equals(before.get(file))) {
  fs.writeFileSync(file,bytes);
  changes.push({file:path.relative(root,file).replaceAll('\\','/'),before_sha256:hash(before.get(file)),after_sha256:hash(bytes)});
}
for(const [file,bytes] of before) {
  if(!plans.some(plan=>plan.file===file)) assert.ok(fs.readFileSync(file).equals(bytes),'Unrelated contract must stay byte-identical');
}
console.log(JSON.stringify({mode:'workspace-approval-bypass',changes,all_other_contracts_byte_identical:true},null,2));
