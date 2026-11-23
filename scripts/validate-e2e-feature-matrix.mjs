// Keep the acceptance inventory complete without confusing registration or a
// passing unit test with an end-to-end execution. Release qualification may opt
// into the stricter completeness gate; ordinary validation reports the gaps.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {repositoryRoot} from './source-layout.mjs';

const read = relative => JSON.parse(fs.readFileSync(path.join(repositoryRoot, relative), 'utf8'));
const matrix = read('docs/e2e-feature-matrix.v1.json');
const routes = read('docs/contracts/legacy-routes.v1.json').routes;
const operations = read('docs/contracts/ui-dtos.v1.json').operations;
const tools = read('docs/contracts/tools.v1.json').tools;
const rows = matrix.rows;
const entries = [
  ['route', routes.map(route => [route.route_id, route.legacy_http_method + ' ' + route.legacy_path])],
  ['ui', operations.map((operation, index) => [`UI${String(index + 1).padStart(3, '0')}`, operation.method])],
  ['tool', tools.map((tool, index) => [`TOOL${String(index + 1).padStart(2, '0')}`, tool.name])],
];
assert.equal(rows.length, routes.length + operations.length + tools.length);
assert.equal(new Set(rows.map(row => row.id)).size, rows.length, 'Duplicate matrix IDs');
const csvIds = fs.readFileSync(path.join(repositoryRoot, 'docs/e2e-feature-matrix.v1.csv'), 'utf8')
  .replace(/^\uFEFF/, '').trimEnd().split(/\r?\n/).slice(1).map(line => line.split(',')[0]);
assert.deepEqual(csvIds, rows.map(row => row.id), 'CSV inventory differs from JSON');
for (const [kind, expected] of entries) {
  assert.deepEqual(rows.filter(row => row.kind === kind).map(row => [row.id, row.function]), expected,
    `${kind}: missing, invented, reordered or renamed source entry`);
}
const stages = ['input', 'admission', 'execution', 'storage', 'reopen', 'display'];
const statuses = ['미검증', '출처 대응', '대응 확인 필요', '부분 경로 통과', 'E2E 통과', '변경점 통과'];
const complete = [];
for (const row of rows) {
  assert.equal(row.qualification_required, row.kind !== 'route', row.id + ': cannot hide an unverified command/tool');
  assert.ok(row.source && row.desktop_entry && row.classification, row.id + ': missing trace');
  assert.ok(Array.isArray(row.evidence), row.id + ': missing evidence array');
  assert.ok(statuses.includes(row.status), row.id + ': unknown result status');
  for (const evidence of row.evidence) {
    assert.ok(evidence.test && evidence.artifact, row.id + ': untraceable evidence');
    assert.ok(Array.isArray(evidence.stages) && evidence.stages.every(stage => stages.includes(stage)),
      row.id + ': invalid execution stages');
  }
  if (row.missing_stages) {
    const observed = new Set(row.evidence.filter(evidence => evidence.result === 'passed')
      .flatMap(evidence => evidence.stages));
    assert.deepEqual(row.missing_stages, stages.filter(stage => !observed.has(stage)),
      row.id + ': missing stages disagree with execution evidence');
  }
  if (['E2E 통과', '변경점 통과'].includes(row.status)) {
    // Separate widget and RPC fixtures can cover all stage names without
    // preserving the real UI envelope/CAS. Qualification needs one joined path.
    assert.ok(row.evidence.some(evidence => evidence.result === 'passed'
      && evidence.connected_path === true
      && stages.every(stage => evidence.stages.includes(stage))),
      row.id + ': no single connected input-to-display execution');
    for (const stage of stages) {
      assert.ok(row.evidence.some(evidence => evidence.stages?.includes(stage)
        && evidence.result === 'passed' && evidence.test && evidence.artifact),
        row.id + ': no successful execution evidence for ' + stage);
    }
    complete.push(row.id);
  }
}
const required = rows.filter(row => row.qualification_required !== false);
const requiredComplete = required.filter(row => complete.includes(row.id));
const result = {inventory_valid: true, routes: routes.length, ui_operations: operations.length,
  tools: tools.length, qualification_entries: required.length,
  e2e_complete: requiredComplete.length, remaining: required.length - requiredComplete.length,
  note: 'Overlapping inventories; counts are not independent features. Static/partial coverage is not E2E.'};
console.log(JSON.stringify(result, null, 2));
if (process.argv.includes('--require-complete')) {
  assert.equal(result.remaining, 0, 'E2E qualification incomplete; do not declare full original-feature parity');
}
