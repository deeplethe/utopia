// 0036 changes the question: score where each column lands, not aggregate SQL.
// Keys are compared against explicit alternatives in the truth file. This is a
// structural score, not a semantic judge or a claim of conversion correctness.
const matches = (got, expected) => (Array.isArray(expected) ? expected : [expected])
  .some((key) => typeof got === "string" && got.toLowerCase() === key.toLowerCase());

export function scoreAlignments(proposals, truth) {
  const results = [];
  for (const table of truth) {
    const candidates = proposals.filter((p) => p.payload.draft.table === table.table);
    const draft = candidates[0]?.payload.draft;
    results.push({ table: table.table, column: null, right: candidates.length === 1 && matches(draft?.class, table.class) });
    for (const expected of table.columns) {
      const columns = draft?.columns.filter((c) => c.column === expected.column) ?? [];
      const column = columns[0];
      const right = expected.omitted
        ? columns.length === 0 && draft?.omitted.some((c) => c.column === expected.column && c.reason.trim()) === true
        : columns.length === 1 && matches(column.class, expected.class) && matches(column.property, expected.property)
          && (expected.target_class ? matches(column.target_class, expected.target_class) : column.target_class == null && column.expression != null);
      results.push({ table: table.table, column: expected.column, right });
    }
  }
  return { checks: results.length, right: results.filter((r) => r.right).length, missed: results.filter((r) => !r.right) };
}
