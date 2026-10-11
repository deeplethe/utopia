# Fixed namesake cases

This is the test-only measurement requested in #1136 and the reviews of #1192
and #1199. It runs real extraction/resolution and adjudication against local,
fixed chat replies (streaming extraction and JSON adjudication). It makes no
provider request and needs no API key or embedding download. It does not introduce an identity gate or an unresolved
entity lifecycle.

Each source directory identifies an employee independently of the model reply.
E17 and F42 are different people with the same name and employer; two E17
sources concern one person. The source-to-identity labels are the oracle, not
canonical names, model confidence or ontology uniqueness declarations. The
graph contains ordinary open employer facts, with no declared unique relation.

| Case | Source identities | Script | Gold identity outcome |
|---|---|---|---|
| `resolver_distinct` | E17, F42 | Extract a bare Alex Lee mention with two existing candidates | Separate source fact owners |
| `adjudicator_distinct_same` | E17, F42 | High-confidence `same` | Separate source fact owners |
| `adjudicator_true_same` | E17, E17 | High-confidence `same` | One source fact owner |
| `adjudicator_distinct_different` | E17, F42 | High-confidence `different` | Separate source fact owners |

The resolver case uses two bare-name candidates and fixed profile vectors.
The first source's holder has `[1,0,0]`; the other candidate has `[0,1,0]` and
no attached facts. The new mention has context `[1,0,0]`, so the wrong holder
has the uniquely highest score. This tests profile preference overriding
source identity, not the existing equal-score tie guard or a degree tie-break.

Every case runs in both source/seed orders, each in a fresh KB. The output records
source identities, actual fact owners, gold violations, surviving entities,
pending human reviews and exact scripted request counts. Ownership follows
the moved subjects of source facts, including duplicate facts invalidated by
merge deduplication; dropping those would lose one source from the measurement.
This measures source identity attribution, not live-fact coverage or query recall. Unexpected/missing
requests, incomplete extraction, missing source ownership and database errors
are hard failures, not accepted identity debt.

After migrating a test database, run:

```sh
UTOPIA_TEST_REQUIRE_DB=1 cargo test -p utopia-server namesake_scripted_cases -- --nocapture
UTOPIA_TEST_REQUIRE_DB=1 UTOPIA_NAMESAKE_STRICT=1 cargo test -p utopia-server namesake_scripted_cases -- --nocapture
```

Both use `UTOPIA_DATABASE_URL`, as the workspace tests do. The strict mode fails
on every gold violation. The default CI mode has an explicit debt ceiling:
at most one `false_merge` in each order of the first two cases, and no identity
violations in the controls. Improvements are allowed; new/worse violations fail.
Human-review growth is reported and checked separately as a budget regression,
not treated as an identity gold label. No tests are ignored and no assertion
requires a wrong merge to persist.

CI executes all cases, publishes a table of gold and budget violations, and
preserves the measurement log. A green budget check means no regression against
the declared ceiling; it does **not** mean all identities are correct. Reviewers
can use the unchanged gold labels and strict command to judge a smaller fix.

These are small synthetic cases, not a replacement for the real-model identity
bench or a proposed general rule that parses employee numbers from prose.
