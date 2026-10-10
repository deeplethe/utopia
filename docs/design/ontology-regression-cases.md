# Property regression cases — 0061 cut 2

This first slice follows the [scope agreed in #1103](https://github.com/deeplethe/utopia/issues/1103#issuecomment-6063201562).
A case remembers a source statement, its expected property and direction, the person
who added it, and whether it came from `adoption` or `person`. Class cases and the
workbench view belong to later slices.

## Adding an expectation

An Editor can post to `/api/v1/kbs/{kb_id}/ontology/regressions`:

```json
{
  "statement_id": "<open statement UUID>",
  "relation_type_id": "<property UUID>",
  "direction": "forward"
}
```

The statement and property must belong to that knowledge base. Entity objects use
relation properties; literal objects use attribute properties, with forward direction.
The server attributes the case to the authenticated person. It does not accept an
actor or origin from the caller. Repeating the same expectation keeps the original
attribution and returns the same case ID.

Property proposals preserve the IDs of the actual example statements offered to the
agent. Adoption writes cases for those examples, with the proposal's direction
(`forward` when no reverse direction was proposed). It does not substitute other
statements with the same phrase. Older proposals resolve their stored example quote
within the stored shape. Removed examples are skipped. The case insertion and the
proposal's transition to adopted commit together.

## Checking a case

The existing phrase alignment pipeline supplies the result. A committed binding
decision updates cases with the same live statement signature in the same transaction.
For agent decisions, a case passes only when both property and direction match.
`none` and `undecided` do not match. A failed model call leaves the previous comparison
and its timestamp alone.

A signature bound by a person always passes, as specified in #1103. The result marks
`human_bound: true`, alongside the actual binding and its decision time. This means
alignment preserves a person's binding; it is not an independent model quality check.
Checking these bindings does not require a model configuration.

There is one latest comparison per case, with its check time. Before a binding exists,
the comparison is absent. Deleting a source statement or expected property deletes its
case through knowledge-base-scoped foreign keys. Invalidated statements are excluded
from comparisons.

No additional evaluator, model call, evaluation queue, or version identity is introduced.
These cases observe alignment; they do not change its decisions, materialized facts,
or the person's expectation. Scripted binding tests validate this comparison contract.
A live-model accuracy benchmark is a separate exercise, not required to test this slice.
