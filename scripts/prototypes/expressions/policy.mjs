// Proposed declaration policy, deliberately outside the production editor/API.
export const MAX_DEPTH = 4; // root=0, checked against authenticated API fixtures
const known = new Set(['USD', 'EUR', 'm', 'kg', 's']);
const numeric = /^[-+]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][-+]?\d+)?$/;
export function declaration(attribute, allowUnitless = false) {
  if (!attribute || attribute.datatype !== 'number') throw Error('numeric_attribute_required');
  if (attribute.unit === '1' && allowUnitless) return '1';
  if (!known.has(attribute.unit)) throw Error('unknown_unit');
  return attribute.unit;
}
export function build(draft, attributes, options = {}, depth = 0) {
  if (depth > MAX_DEPTH) throw Error('expression_too_deep');
  if (!draft || draft.kind === 'empty') throw Error('incomplete_expression');
  if (draft.kind === 'attr') {
    const attr = attributes.find(a => a.id === draft.id);
    return { ast: { attr: draft.id }, unit: declaration(attr, options.allowUnitless), reads: [draft.id] };
  }
  if (draft.kind === 'const') {
    const text = draft.raw.trim();
    if (!numeric.test(text) || !Number.isFinite(Number(text))) throw Error('finite_number_required');
    return { ast: { const: Number(text) }, unit: '1', reads: [] };
  }
  if (draft.kind !== 'arith' || !['add','sub','mul','div'].includes(draft.op)) throw Error('unsupported_expression');
  const l=build(draft.l,attributes,options,depth+1), r=build(draft.r,attributes,options,depth+1);
  let unit;
  if (['add','sub'].includes(draft.op) && l.unit === r.unit) unit=l.unit;
  if (draft.op === 'mul' && (l.unit === '1' || r.unit === '1')) unit=l.unit === '1' ? r.unit : l.unit;
  if (draft.op === 'div' && (r.unit === '1' || r.unit === l.unit)) unit=r.unit === '1' ? l.unit : '1';
  if (!unit) throw Error('incompatible_units');
  return { ast: {op:draft.op,l:l.ast,r:r.ast}, unit, reads:[...new Set([...l.reads,...r.reads])] };
}
export function conclude(draft, target, attributes, options = {}) {
  const result=build(draft,attributes,options);
  if (!result.reads.length) throw Error('constant_expression');
  if (declaration(attributes.find(a => a.id===target),options.allowUnitless)!==result.unit) throw Error('target_unit_mismatch');
  return result.ast;
}
export function reopen(ast, depth=0) {
  if (!ast || typeof ast !== 'object' || Array.isArray(ast) || depth>MAX_DEPTH) return null;
  const keys=Object.keys(ast).sort().join(',');
  if (keys==='attr' && typeof ast.attr==='string') return {kind:'attr',id:ast.attr};
  // Legacy string constants stay metadata-only; no silent conversion on reopen.
  if (keys==='const' && typeof ast.const==='number' && Number.isFinite(ast.const)) return {kind:'const',raw:String(ast.const)};
  if (keys!=='l,op,r' || !['add','sub','mul','div'].includes(ast.op)) return null;
  const l=reopen(ast.l,depth+1),r=reopen(ast.r,depth+1);
  return l && r ? {kind:'arith',op:ast.op,l,r} : null;
}
export function preview(ast, attributes) {
  if ('attr' in ast) { const a=attributes.find(a=>a.id===ast.attr); return a ? `${a.label} [${a.key}]` : ast.attr; }
  if ('const' in ast) return String(ast.const);
  return `(${preview(ast.l,attributes)} ${{add:'+',sub:'−',mul:'×',div:'÷'}[ast.op]} ${preview(ast.r,attributes)})`;
}
export function patch(original, metadata, definition) {
  const result={name:metadata.name,description:metadata.description};
  // The editor only supplies definition after explicit opt-in. Group identities
  // and UUIDs are copied as a whole, never rebuilt from labels or flattened.
  if (definition && JSON.stringify(definition)!==JSON.stringify(original)) Object.assign(result,structuredClone(definition));
  return result;
}
