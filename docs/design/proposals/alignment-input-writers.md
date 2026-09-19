# R3 semantic writer inventory

Baseline e894fa1. Graph definition search, source snippets, inbound traces, and SQL-literal graph search were cross-checked. Graph Rust caller coverage is incomplete (some known routes return no callers), so an empty caller result is not proof of no use. No existing per-KB revision/bump protocol was found in these paths. Tests below are required future revision tests, not already-passing revision evidence.

| Actual entry/helper | Data / transaction boundary today | Required revision test |
|---|---|---|
| ontology::create_entity_type / update_entity_type | entity_types statement then separate set_parents calls through pool | Definition + parents + bump commit/rollback together; normal edit used in R3 red test |
| ontology::set_parents / set_parents_bulk | Deletes/inserts parents; bulk insert; no revision, standalone helper does not update class timestamp | Add/remove/non-primary parent invalidates dependent input; rollback |
| ontology::delete_entity_type | Attribute deletion then class deletion as separate statements; cascades edges | Legal deletion advances once; failed deletion has no partial bump/change |
| ontology::create_relation_type / update_relation_type | relation_types write then domain/range writes; qualifiers edited separately | Definition/domain/range atomic snapshot, no partially updated candidate |
| ontology::set_domains_ranges | Separate DELETE/INSERT for domain then range | One shared transaction; no midpoint snapshot |
| ontology::delete_relation_type | Usage check and scoped delete | Allowed deletion invalidates candidates; in-use rejection unchanged |
| ontology::create_entity_type_with_iri / update_type_from_import / adopt_iri_onto_key | Imported class statement; timestamps updated in update paths | Imported definition counted, no-op/conflict policy explicit |
| ontology::create_attribute_with_iri / create_relation_with_iri | Property insert then separate domain/range writes | All imported semantic rows and bump atomic |
| ontology::update_relation_from_import | Property label/description update, then domain/range; update lacks explicit updated_at | Revision must cover this path even when old timestamp logic does not |
| ontology::create_entity_types_bulk / create_relation_types_bulk | Bulk UNNEST inserts with ON CONFLICT DO NOTHING | No-op import versus newly inserted candidate; failed batch rollback |
| ontology::link_domains_ranges_bulk | Dynamic INSERT into domain/range join tables | Pack second-pass link cannot bypass bump |
| ontology::link_property_axioms_bulk | Dynamic UPDATE inverse_of/sub_property_of | Axiom dependency revision; partial imported references |
| ontology::set_disjoint_for / set_disjoint_bulk | Local transaction for replace; standalone bulk insert | Include if consumed semantics; common outer transaction required |
| ontology::set_relation_qualifiers | Validate then separate delete/insert qualifiers | Scope decision explicit; version if consumed by alignment/materialization |
| ontology::attribute_from_unused_relation | Change relation kind then domain/range separately | Conversion changes compatibility and participates in revision |
| owl_import::apply | Orchestrates bulk classes/parents/properties/axioms/domains and audit/jobs using pool | Whole import or explicitly staged unpublished revision; no half-import visibility |
| api::kbs::install_packs -> owl_import::apply + relink_domains_ranges | Per-pack first pass, cross-pack links second pass | Second pass advances/participates; no route-only bump |
| api::ontology_routes::{create/update/delete entity/relation,apply_import,adopt_predicate,adopt_attribute_core,type_resolution_apply} | Authenticated orchestrators invoke store helpers and schedule work | Reuse protected writers; adoption/rejection and enqueue atomicity audited separately |
| names::ensure_known_as | Lazy builtin relation insert | Include candidate-set additions or prove intentional exclusion |
| business_rules::ensure_is_a | Lazy derived typing property insert | Same; do not bump during read-only evaluation by accident |
| mappings::ensure_concept_types | Direct builtin entity_type inserts outside ontology module | Must join protected writer API if included in candidate set |
| ontology::set_type_embeddings | Separate derived-vector columns, own transaction | Normally exclude from semantic revision; vector retrieval must refer to captured definitions |
| ontology::{record_import,save_proposals,decide_proposal} | Metadata/review state, no semantic schema modification by itself | Do not bump merely on draft/rejection; real adoption writer does |

Also inventory migrations/restore before enabling a mixed-version deployment: schema migrations seed/backfill data and backup restore must restore or invalidate revision metadata coherently. Operational offline restore is not an ordinary live editing transaction. Future writers must use the same store-level protocol; a list alone does not enforce completeness. No production revision implementation or all-writer dynamic proof is claimed.
