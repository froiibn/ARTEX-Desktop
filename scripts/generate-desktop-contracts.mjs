// Rebuild frozen source inventories. No database, tools, or network calls.
import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import { createRequire } from 'node:module';
import { repositoryRoot, requireOriginalRoot } from './source-layout.mjs';
const root = requireOriginalRoot();
const out = path.join(repositoryRoot, 'docs/contracts');
fs.mkdirSync(out, { recursive: true });
const read = p => fs.readFileSync(path.join(root, p), 'utf8');
const hash = s => crypto.createHash('sha256').update(s).digest('hex');
const save = (p, data) => fs.writeFileSync(path.join(out, p), JSON.stringify(data, null, 2) + '\n');
const source = read('db/schema.sql');
const supplemental = ['db/commands.go', 'db/llm_usage.go', 'traffic/traffic.go'].flatMap(p => [...read(p).matchAll(/`([^`]+)`/g)].filter(m => /CREATE TABLE|ALTER TABLE/.test(m[1])).map(m => ({ path: p, sql: m[1] })));
const allSQL = [source, ...supplemental.map(x => x.sql)].join('\n');
// SQL comments outside quoted strings; balanced delimiters retain nested CHECKs.
const sql = allSQL.replace(/'(?:''|[^'])*'|--[^\r\n]*/g, s => s.startsWith('--') ? '' : s);
function split(s, delimiter = ',') {
  const result = []; let depth = 0, quoted = false, start = 0;
  for (let i = 0; i < s.length; i++) {
    if (s[i] === "'") { if (quoted && s[i + 1] === "'") { i++; continue; } quoted = !quoted; }
    if (quoted) continue;
    if (s[i] === '(' || s[i] === '[') depth++;
    if (s[i] === ')' || s[i] === ']') depth--;
    if (!depth && s[i] === delimiter) { result.push(s.slice(start, i).trim()); start = i + 1; }
  }
  result.push(s.slice(start).trim()); return result.filter(Boolean);
}
function endParen(s, start) {
  let d = 0, q = false;
  for (let i = start; i < s.length; i++) {
    if (s[i] === "'") { if (q && s[i + 1] === "'") { i++; continue; } q = !q; }
    if (!q && s[i] === '(') d++;
    if (!q && s[i] === ')' && --d === 0) return i;
  }
  throw Error('Unclosed SQL declaration');
}
function column(text) {
  const m = text.match(/^(\w+)\s+(BIGSERIAL|SERIAL|BIGINT|INTEGER|INT|TEXT|BOOLEAN|JSONB|TIMESTAMPTZ|CIDR|DOUBLE\s+PRECISION|REAL|BYTEA|BLOB)(\[\])?(?=\s|$)/i);
  if (!m) return null;
  const reference = text.match(/REFERENCES\s+(\w+)\s*\((\w+)\)(?:\s+ON DELETE (CASCADE|SET NULL|RESTRICT))?/i);
  return { name: m[1], sql_type: (m[2] + (m[3] || '')).toUpperCase(), nullable: !/NOT NULL|PRIMARY KEY/i.test(text), declaration: text.replace(/\s+/g, ' '), ...(reference ? { reference: { collection: reference[1], field: reference[2], on_delete: reference[3] || 'RESTRICT' } } : {}) };
}
const tables = [];
for (const m of sql.matchAll(/CREATE TABLE IF NOT EXISTS (\w+)\s*\(/g)) {
  const start = m.index + m[0].length - 1;
  const clauses = split(sql.slice(start + 1, endParen(sql, start)));
  tables.push({ name: m[1], source: supplemental.find(x => x.sql.includes(`CREATE TABLE IF NOT EXISTS ${m[1]} (`))?.path || 'db/schema.sql', columns: clauses.map(column).filter(Boolean), constraints: clauses.filter(c => !column(c)) });
}
for (const m of sql.matchAll(/ALTER TABLE (\w+) ADD COLUMN (?:IF NOT EXISTS )?([^;]+);/g)) {
  const t = tables.find(t => t.name === m[1]); const c = column(m[2].trim());
  if (!t || !c) throw Error('Unrecognized ADD COLUMN: ' + m[0]);
  if (!t.columns.some(x => x.name === c.name)) t.columns.push(c);
}
for (const m of sql.matchAll(/ALTER TABLE (\w+) ALTER COLUMN (\w+) SET (DEFAULT [^;]+|NOT NULL);/g)) {
  const c = tables.find(t => t.name === m[1])?.columns.find(c => c.name === m[2]);
  if (!c) throw Error('Unknown ALTER target: ' + m[0]);
  c.final_alterations ??= []; c.final_alterations.push(m[3].replace(/\s+/g, ' '));
  if (m[3] === 'NOT NULL') c.nullable = false;
}
const indices = [...sql.matchAll(/CREATE (UNIQUE )?INDEX IF NOT EXISTS ([\s\S]*?);/g)].map(m => m[0].replace(/\s+/g, ' '));
const constraints = [...sql.matchAll(/ALTER TABLE (\w+) ADD CONSTRAINT ([\s\S]*?);/g)].map(m => ({ collection: m[1], declaration: m[0].replace(/\s+/g, ' ') }));
const hostCollections = new Set(['llm_profile_health', 'mcp_tools_cache', 'server_logs']);
for (const t of tables) {
  t.destination = hostCollections.has(t.name) ? 'rebuildable_projection' : 'workspace_collection';
  t.columns.forEach(c => {
    const array = c.sql_type.endsWith('[]'); const base = c.sql_type.replace('[]', '');
    c.desktop_type = c.reference || c.name === 'id' && /SERIAL/.test(base) ? 'typed_id' : /BIGINT|BIGSERIAL/.test(base) ? 'decimal_string' : /SERIAL|INT/.test(base) ? 'int32' : /BOOLEAN/.test(base) ? 'boolean' : /JSONB/.test(base) ? 'json_value' : /PRECISION|REAL/.test(base) ? 'finite_number' : /BLOB|BYTEA/.test(base) ? 'blob_ref' : 'string';
    if (array) c.desktop_type += '[]';
    if (/TIMESTAMPTZ/.test(base)) c.desktop_type = 'utc_timestamp';
    if (['task_ids', 'asset_ids', 'source_task_ids', 'member_ids'].includes(c.name)) c.desktop_type = 'typed_id[]';
    if (t.source === 'traffic/traffic.go' && ['ts', 'req_len', 'resp_len'].includes(c.name)) c.desktop_type = 'decimal_string';
    if (t.name === 'agents' && c.name === 'current_prompt_id') { c.reference = { collection: 'agent_prompts', field: 'id', on_delete: 'SET NULL' }; c.desktop_type = 'typed_id'; }
    c.migration = 'preserve';
    if (t.name === 'assets' && c.name === 'task_ids') c.migration = 'derive_from_task_asset_links';
    if ((t.name === 'llm_profiles' && c.name === 'api_key') || (t.name === 'assets' && c.name === 'auth') || (t.name === 'mcp_servers' && c.name === 'env') || (t.name === 'notification_channels' && c.name === 'config')) c.migration = 'split_secret_references';
    if (t.name === 'settings') c.migration = 'classify_setting_key';
  });
}
save('data-model.v1.json', { contract_version: '1.0', source: 'db/schema.sql', source_sha256: hash(source), supplemental_sources: supplemental.map(x => x.path).filter((v, i, a) => a.indexOf(v) === i), tables, indices, additional_constraints: constraints, derived_virtual_tables: ['ex_fts'], sql_functions_and_triggers: [...sql.matchAll(/CREATE(?: OR REPLACE)? (FUNCTION|TRIGGER) (\w+)/g)].map(m => ({ kind: m[1], name: m[2] })) });
const files = [];
function walk(dir) { for (const x of fs.readdirSync(path.join(root, dir), { withFileTypes: true })) { const p = `${dir}/${x.name}`; if (x.isDirectory()) walk(p); else if (/\.go$/.test(p) && !p.endsWith('_test.go')) files.push(p); } }
for (const dir of ['agent','cmd','config','db','enrich','evidence','guard','intercept','llmpool','llmrec','mcphttp','norma','notify','report','selfupdate','server','sidequestion','traffic']) walk(dir);
for (const x of fs.readdirSync(root,{withFileTypes:true})) if (x.isFile() && x.name.endsWith('.go') && !x.name.endsWith('_test.go')) files.push(x.name);
const goFiles = files.map(p => ({ path: p, text: read(p) }));
function handlerSource(name) {
  for (const f of goFiles.filter(f => f.path.startsWith('server/'))) {
    const r = new RegExp(`func \\(s \\*Server\\) ${name}\\(`).exec(f.text);
    if (r) { const tail = f.text.slice(r.index); const next = /\nfunc /.exec(tail.slice(1)); const text = next ? tail.slice(0, next.index + 1) : tail; return { file: f.path, line: f.text.slice(0, r.index).split('\n').length, sha256: hash(text), go_contract: text.trim() }; }
  }
  throw Error('Handler not found: ' + name);
}
const declarations = [...read('server/server.go').matchAll(/mux\.HandleFunc\("([A-Z]+) ([^"]+)", s\.(\w+)\)/g)].map(m => ({ method:m[1], url:m[2], handler:m[3], registration:'server/server.go' }));
// Expand the two helper registrars; fail validation if their source changes.
const findingRegistration = read('server/finding_traffic.go');
const findingBase = findingRegistration.match(/base := "([^"]+)"/)[1];
for (const m of findingRegistration.matchAll(/mux\.HandleFunc\("([A-Z]+) "\+base(?:\+"([^"]+)")?, s\.(\w+)\)/g)) declarations.push({ method:m[1], url:findingBase+(m[2]||''), handler:m[3], registration:'server/finding_traffic.go' });
const sideRegistration = read('server/side_questions.go');
const sidePaths = [...sideRegistration.matchAll(/\{"(\/api\/[^"\n]+)", "(conversation|main|worker)"\}/g)];
for (const m of sidePaths) for (const method of ['GET','POST','DELETE']) declarations.push({ method, url:m[1]+'/side-questions', handler:'handleSideQuestions', parent_kind:m[2], registration:'server/side_questions.go' });
for (const m of sideRegistration.matchAll(/mux\.HandleFunc\("([A-Z]+) ([^"]+)", s\.(\w+)\)/g)) declarations.push({ method:m[1], url:m[2], handler:m[3], registration:'server/side_questions.go' });
const routes = declarations.map(({method,url,handler,...registration}) => {
  const disposition = url.startsWith('/api/auth/') ? 'removed_os_session' : url.startsWith('/api/update/') ? 'host_signed_update' : /\/stream$|\/events$/.test(url) || handler === 'chat' ? 'event_subscription' : /upload|download|\/blob|\/body$|\/export$/.test(url) ? 'blob_capability' : 'engine';
  return { route_id:'route_'+hash(method+' '+url).slice(0,16), legacy_http_method:method, legacy_path:url, handler, rpc_method:`legacy.${handler}`, disposition, mutates:method !== 'GET', ...registration, ...handlerSource(handler) };
});
save('legacy-routes.v1.json', { contract_version: '1.0', routes });
// Freeze expanded TypeScript DTOs as JSON Schema; IDs are converted by v1 rules.
const require = createRequire(import.meta.url);
const ts = require(path.join(root, 'web/node_modules/typescript/lib/typescript.js'));
const config = ts.readConfigFile(path.join(root, 'web/tsconfig.json'), ts.sys.readFile);
const parsed = ts.parseJsonConfigFileContent(config.config, ts.sys, path.join(root, 'web'));
const program = ts.createProgram(parsed.fileNames, { ...parsed.options, strictNullChecks: true });
const checker = program.getTypeChecker(); const apiFile = program.getSourceFile(path.join(root, 'web/src/lib/api.ts'));
const definitions = {}; const ids = new Map(); let count = 0;
function schema(type) {
  const F = ts.TypeFlags;
  if (type.flags & (F.Any | F.Unknown)) return {};
  if (type.flags & (F.Undefined | F.Void | F.Never)) return { 'x-undefined': true };
  if (type.flags & F.Null) return { type: 'null' };
  if (type.isUnion()) return { anyOf: type.types.filter(t => !(t.flags & F.Undefined)).map(schema) };
  if (type.isStringLiteral()) return { const: type.value };
  if (type.isNumberLiteral()) return { const: type.value };
  if (type.flags & F.BooleanLiteral) return { const: type.intrinsicName === 'true' };
  if (type.flags & F.StringLike) return { type: 'string' };
  if (type.flags & F.NumberLike) return { type: 'number' };
  if (type.flags & F.BooleanLike) return { type: 'boolean' };
  if (checker.isArrayType(type)) return { type: 'array', items: schema(checker.getTypeArguments(type)[0]) };
  const name = type.symbol?.getName();
  if (['File', 'Blob', 'AbortSignal', 'FormData'].includes(name)) return { type: 'string', 'x-native-capability': name };
  if (ids.has(type)) return { $ref: `#/$defs/${ids.get(type)}` };
  const id = `T${++count}`; ids.set(type, id); definitions[id] = {};
  const properties = {}, required = [];
  for (const prop of checker.getPropertiesOfType(type)) {
    if (prop.getName().startsWith('__')) continue;
    const decl = prop.valueDeclaration || prop.declarations?.[0] || apiFile;
    properties[prop.getName()] = schema(checker.getTypeOfSymbolAtLocation(prop, decl));
    if (!(prop.flags & ts.SymbolFlags.Optional)) required.push(prop.getName());
  }
  const index = checker.getIndexTypeOfType(type, ts.IndexKind.String);
  definitions[id] = { type: 'object', properties, required, additionalProperties: index ? schema(index) : false, 'x-source-type': checker.typeToString(type, undefined, ts.TypeFormatFlags.NoTruncation) };
  return { $ref: `#/$defs/${id}` };
}
let apiNode;
function find(n) { if (ts.isVariableDeclaration(n) && n.name.getText(apiFile) === 'api') apiNode = n.initializer; ts.forEachChild(n, find); }
find(apiFile); if (!apiNode || !ts.isObjectLiteralExpression(apiNode)) throw Error('api object not found');
const operations = [];
for (const prop of apiNode.properties) {
  if (!ts.isPropertyAssignment(prop)) continue;
  const fn = prop.initializer; if (!ts.isArrowFunction(fn) && !ts.isFunctionExpression(fn)) continue;
  const sig = checker.getSignatureFromDeclaration(fn); const returnType = checker.getReturnTypeOfSignature(sig);
  const unwrapped = checker.getPromisedTypeOfPromise(returnType) || returnType;
  const parameters = fn.parameters.map(p => ({ name: p.name.getText(apiFile), optional: !!p.questionToken || !!p.initializer, schema: schema(checker.getTypeAtLocation(p)) }));
  operations.push({ method: `ui.${prop.name.getText(apiFile).replaceAll('"', '')}`, parameters, result_schema: schema(unwrapped), source_line: apiFile.getLineAndCharacterOfPosition(prop.pos).line + 1, legacy_adapter: prop.getText(apiFile) });
}
save('ui-dtos.v1.json', { $schema: 'https://json-schema.org/draft/2020-12/schema', contract_version: '1.0', source_sha256: hash(read('web/src/lib/api.ts')), representation: 'legacy expanded DTOs; apply normative typed-ID, timestamp and secret overlays', operations, $defs: definitions });
save('source-manifest.v1.json', { contract_version: '1.0', files: [...files, 'db/schema.sql', 'web/src/lib/api.ts', 'web/src/lib/types.ts', 'go.mod', 'norma/go.mod'].sort().map(p => ({ path: p, sha256: hash(fs.readFileSync(path.join(root, p))) })) });
console.log(`Frozen ${tables.length} tables, ${tables.reduce((n, t) => n + t.columns.length, 0)} columns, ${routes.length} routes, ${operations.length} UI operations, ${count} DTO definitions.`);
