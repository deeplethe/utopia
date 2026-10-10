//! A table's ontology declarations and column bindings are one human decision (0036).
//! Exploration stores an expression tree with provisional attribute identities. Adoption
//! resolves those keys again in its transaction and substitutes the actual identities;
//! two tables can therefore propose the same new attribute without creating it twice.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::PgPool;
use std::collections::{BTreeMap, HashMap, HashSet};
use utopia_core::models::{EntityType, RelationAxioms, RelationType};
use utopia_core::{AppError, AppResult};
use utopia_reason::rules::Expr;
use uuid::Uuid;

pub const SECTION: &str = "table_alignments";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClassDeclaration {
    pub key: String,
    pub label: String,
    pub description: String,
    #[serde(default)]
    pub parents: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PropertyDeclaration {
    pub key: String,
    pub label: String,
    pub description: String,
    pub domains: Vec<String>,
    #[serde(default)]
    pub ranges: Vec<String>,
    pub datatype: Option<String>,
    pub unit: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColumnAlignment {
    pub column: String,
    pub class: String,
    pub property: String,
    pub target_class: Option<String>,
    pub expression: Option<Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OmittedColumn {
    pub column: String,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Draft {
    pub table: String,
    pub class: String,
    pub summary: String,
    #[serde(default)]
    pub entity_types: Vec<ClassDeclaration>,
    #[serde(default)]
    pub attribute_types: Vec<PropertyDeclaration>,
    #[serde(default)]
    pub relation_types: Vec<PropertyDeclaration>,
    pub columns: Vec<ColumnAlignment>,
    #[serde(default)]
    pub omitted: Vec<OmittedColumn>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Proposal {
    pub version: Uuid,
    pub source_id: Uuid,
    pub source: String,
    pub draft: Draft,
    /// Raw column -> provisional attribute identity. Each conversion keeps its
    /// own input bindings when these become actual ontology attribute IDs.
    pub attribute_ids: BTreeMap<String, Uuid>,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct ProposalView {
    pub id: Uuid,
    pub key: String,
    pub status: String,
    pub payload: Value,
}

fn invalid(message: impl Into<String>) -> AppError {
    AppError::invalid("bad_table_alignment", message)
}

pub fn key(source_id: Uuid, table: &str) -> String {
    format!("{source_id}:{table}")
}

// Validate against ontology keys, including inherited domains/ranges. Reusing an
// existing key never authorizes exploration to change its meaning or signature.
struct Catalogue {
    classes: HashMap<String, Vec<String>>,
    properties: HashMap<String, (String, PropertyDeclaration)>,
}

impl Catalogue {
    fn new(classes: &[EntityType], properties: &[RelationType], draft: &Draft) -> AppResult<Self> {
        let keys: HashMap<Uuid, String> = classes.iter().map(|c| (c.id, c.key.clone())).collect();
        let names = |ids: &[Uuid]| ids.iter().filter_map(|id| keys.get(id).cloned()).collect();
        let mut out = Self {
            classes: classes
                .iter()
                .map(|c| (c.key.clone(), names(&c.parents)))
                .collect(),
            properties: properties
                .iter()
                .map(|p| {
                    (
                        p.key.clone(),
                        (
                            p.kind.clone(),
                            PropertyDeclaration {
                                key: p.key.clone(),
                                label: p.label.clone(),
                                description: p.description.clone(),
                                domains: names(&p.domains),
                                ranges: names(&p.ranges),
                                datatype: p.datatype.clone(),
                                unit: p.unit.clone(),
                            },
                        ),
                    )
                })
                .collect(),
        };
        let mut declared = HashSet::new();
        for c in &draft.entity_types {
            validate_declaration(&c.key, &c.label)?;
            if !declared.insert(&c.key) {
                return Err(invalid("A class is declared twice"));
            }
            if let Some(parents) = out.classes.get(&c.key) {
                if !same_set(parents, &c.parents) {
                    return Err(invalid(format!("Class {} changed; explore again", c.key)));
                }
            } else {
                out.classes.insert(c.key.clone(), c.parents.clone());
            }
        }
        for c in &draft.entity_types {
            for parent in &c.parents {
                out.class(parent)?;
            }
            if c.parents.iter().any(|p| out.is_a(p, &c.key)) {
                return Err(invalid("Proposed classes form a cycle"));
            }
        }
        declared.clear();
        for (kind, list) in [
            ("attribute", &draft.attribute_types),
            ("relation", &draft.relation_types),
        ] {
            for p in list {
                validate_declaration(&p.key, &p.label)?;
                if !declared.insert(&p.key) {
                    return Err(invalid("A property is declared twice"));
                }
                for c in p.domains.iter().chain(&p.ranges) {
                    out.class(c)?;
                }
                if p.domains.is_empty()
                    || (kind == "attribute"
                        && (!p.ranges.is_empty()
                            || !matches!(
                                p.datatype.as_deref(),
                                Some("text" | "number" | "date" | "bool")
                            )))
                    || (kind == "relation"
                        && (p.ranges.is_empty() || p.datatype.is_some() || p.unit.is_some()))
                {
                    return Err(invalid(format!("Invalid signature for {}", p.key)));
                }
                if let Some((old_kind, old)) = out.properties.get(&p.key) {
                    if old_kind != kind
                        || !same_set(&old.domains, &p.domains)
                        || !same_set(&old.ranges, &p.ranges)
                        || old.datatype != p.datatype
                        || old.unit != p.unit
                    {
                        return Err(invalid(format!(
                            "Property {} changed; explore again",
                            p.key
                        )));
                    }
                } else {
                    out.properties
                        .insert(p.key.clone(), (kind.into(), p.clone()));
                }
            }
        }
        Ok(out)
    }

    fn class(&self, key: &str) -> AppResult<()> {
        self.classes
            .contains_key(key)
            .then_some(())
            .ok_or_else(|| invalid(format!("Unknown class {key}")))
    }

    fn is_a(&self, child: &str, ancestor: &str) -> bool {
        let mut pending = vec![child];
        let mut visited = HashSet::new();
        while let Some(key) = pending.pop() {
            if key == ancestor {
                return true;
            }
            if visited.insert(key) {
                if let Some(parents) = self.classes.get(key) {
                    pending.extend(parents.iter().map(String::as_str));
                }
            }
        }
        false
    }

    fn validate(&self, proposal: &Proposal) -> AppResult<()> {
        let d = &proposal.draft;
        self.class(&d.class)?;
        if d.table.trim().is_empty()
            || d.summary.trim().is_empty()
            || d.columns.is_empty()
            || d.columns.len() > 200
        {
            return Err(invalid(
                "An alignment needs a table, summary and between one and 200 columns",
            ));
        }
        let mut columns = HashSet::new();
        let mut attributes = HashSet::new();
        for c in &d.columns {
            self.class(&c.class)?;
            if c.column.is_empty() || !columns.insert(&c.column) {
                return Err(invalid("A column is aligned twice"));
            }
            let (kind, p) = self
                .properties
                .get(&c.property)
                .ok_or_else(|| invalid(format!("Unknown property {}", c.property)))?;
            if !p.domains.is_empty() && !p.domains.iter().any(|domain| self.is_a(&c.class, domain))
            {
                return Err(invalid(format!(
                    "{} is not an attribute or relation of {}",
                    c.property, c.class
                )));
            }
            match (kind.as_str(), &c.target_class, &c.expression) {
                ("attribute", None, Some(tree)) => {
                    attributes.insert(&c.column);
                    let expr = Expr::from_json(tree)
                        .map_err(|e| AppError::invalid(e.code(), e.message()))?;
                    let mut reads = Vec::new();
                    expr.predicates(&mut reads);
                    let allowed: HashSet<Uuid> = proposal.attribute_ids.values().copied().collect();
                    if reads.iter().any(|id| !allowed.contains(id)) {
                        return Err(invalid(
                            "An expression reads an attribute without a column binding",
                        ));
                    }
                    let mut bound = HashMap::new();
                    for input in reads {
                        let column = proposal
                            .attribute_ids
                            .iter()
                            .find(|(_, id)| **id == input)
                            .unwrap()
                            .0;
                        let property = &d
                            .columns
                            .iter()
                            .find(|c| &c.column == column && c.target_class.is_none())
                            .ok_or_else(|| invalid("Expression input is not an aligned attribute"))?
                            .property;
                        if let Some(previous) = bound.insert(property, column) {
                            if previous != column {
                                return Err(invalid("One conversion cannot read two columns bound to the same attribute"));
                            }
                        }
                    }
                }
                ("relation", Some(target), None) => {
                    self.class(target)?;
                    if !p.ranges.is_empty()
                        && !p.ranges.iter().any(|range| self.is_a(target, range))
                    {
                        return Err(invalid(format!("{} cannot point to {target}", c.property)));
                    }
                }
                _ => {
                    return Err(invalid(
                        "An attribute needs a conversion; a relation needs a target class",
                    ))
                }
            }
        }
        for omitted in &d.omitted {
            if omitted.reason.trim().is_empty() || !columns.insert(&omitted.column) {
                return Err(invalid(
                    "An omitted column needs a reason and cannot also be aligned",
                ));
            }
        }
        if attributes.len() != proposal.attribute_ids.len()
            || attributes
                .iter()
                .any(|k| !proposal.attribute_ids.contains_key(*k))
            || proposal
                .attribute_ids
                .values()
                .collect::<HashSet<_>>()
                .len()
                != attributes.len()
        {
            return Err(invalid("Every input attribute needs exactly one identity"));
        }
        // A flattened owner need not have its own key column (Address.province
        // is a common example). Recording its class is still useful alignment;
        // producing joins or row identities belongs to the later renderer.
        let used_properties: HashSet<_> = d.columns.iter().map(|c| &c.property).collect();
        if d.attribute_types
            .iter()
            .chain(&d.relation_types)
            .any(|p| !used_properties.contains(&p.key))
        {
            return Err(invalid(
                "A table cannot propose properties that none of its columns uses",
            ));
        }
        Ok(())
    }
}

fn same_set(a: &[String], b: &[String]) -> bool {
    a.iter().collect::<HashSet<_>>() == b.iter().collect::<HashSet<_>>()
}

fn validate_declaration(key: &str, label: &str) -> AppResult<()> {
    if key.is_empty()
        || key.len() > 128
        || label.trim().is_empty()
        || !key
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return Err(invalid(
            "New ontology keys use lowercase letters, digits and underscores and need a label",
        ));
    }
    Ok(())
}

pub async fn prepare(
    pool: &PgPool,
    kb_id: Uuid,
    mut proposal: Proposal,
    compile: impl Fn(&str, &HashMap<String, Uuid>) -> AppResult<Value>,
) -> AppResult<Proposal> {
    let classes = crate::graph::entity_types(pool, kb_id).await?;
    let properties = crate::graph::relation_types(pool, kb_id).await?;
    let catalogue = Catalogue::new(&classes, &properties, &proposal.draft)?;
    proposal.attribute_ids.clear();
    let mut names = HashMap::new();
    for c in &proposal.draft.columns {
        if c.target_class.is_none() {
            let id = *proposal
                .attribute_ids
                .entry(c.column.clone())
                .or_insert_with(Uuid::now_v7);
            names.insert(c.column.clone(), id);
        }
    }
    for c in &mut proposal.draft.columns {
        if let Some(value) = &mut c.expression {
            if let Some(text) = value.as_str() {
                *value = compile(text, &names)?;
            }
        } else if c.target_class.is_none() {
            c.expression = Some(json!({"attr": names[&c.column]}));
        }
    }
    catalogue.validate(&proposal)?;
    Ok(proposal)
}

pub async fn save(pool: &PgPool, kb_id: Uuid, proposal: &Proposal) -> AppResult<bool> {
    let result = sqlx::query(
        "INSERT INTO ontology_proposals(id,kb_id,section,key,payload,proposed_by)
         SELECT $1,$2,'table_alignments',$3,$4,'exploration'
         WHERE EXISTS (SELECT 1 FROM kb_data_sources WHERE kb_id=$2 AND data_source_id=$5)
         ON CONFLICT(kb_id,section,key) DO UPDATE SET payload=EXCLUDED.payload, created_at=now()
         WHERE ontology_proposals.status='open'",
    )
    .bind(Uuid::now_v7())
    .bind(kb_id)
    .bind(key(proposal.source_id, &proposal.draft.table))
    .bind(json!(proposal))
    .bind(proposal.source_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() == 1)
}

pub async fn list(pool: &PgPool, kb_id: Uuid) -> AppResult<Vec<ProposalView>> {
    Ok(sqlx::query_as(
        "SELECT p.id,p.key,p.status,p.payload FROM ontology_proposals p
         JOIN kb_data_sources m ON m.kb_id=p.kb_id AND m.data_source_id::text=p.payload->>'source_id'
         WHERE p.kb_id=$1 AND p.section='table_alignments' ORDER BY p.created_at DESC,p.key",
    ).bind(kb_id).fetch_all(pool).await?)
}

/// Lock the proposal before re-reading its ontology. No pool acquisition is made
/// inside this transaction, so adoption also works with a one-connection pool.
pub async fn decide(
    pool: &PgPool,
    kb_id: Uuid,
    proposal_key: &str,
    version: Uuid,
    adopt: bool,
    actor: Uuid,
) -> AppResult<()> {
    let mut tx = pool.begin().await?;
    let (id, status, payload): (Uuid, String, Value) = sqlx::query_as(
        "SELECT id,status,payload FROM ontology_proposals WHERE kb_id=$1 AND section='table_alignments' AND key=$2 FOR UPDATE",
    ).bind(kb_id).bind(proposal_key).fetch_optional(&mut *tx).await?.ok_or(AppError::NotFound)?;
    if status != "open" {
        return Err(AppError::Conflict(
            "This table has already been reviewed".into(),
        ));
    }
    let proposal: Proposal =
        serde_json::from_value(payload.clone()).map_err(|e| invalid(e.to_string()))?;
    if proposal.version != version {
        return Err(AppError::Conflict(
            "This proposal changed; refresh before reviewing it".into(),
        ));
    }
    // Unmounting races adoption too: keep the mount until the transaction ends.
    let mounted: Option<Uuid> = sqlx::query_scalar(
        "SELECT data_source_id FROM kb_data_sources WHERE kb_id=$1 AND data_source_id=$2 FOR SHARE",
    )
    .bind(kb_id)
    .bind(proposal.source_id)
    .fetch_optional(&mut *tx)
    .await?;
    if mounted.is_none() {
        return Err(invalid("The data source is no longer mounted"));
    }
    if adopt {
        let classes = crate::graph::entity_types(&mut *tx, kb_id).await?;
        let properties = crate::graph::relation_types(&mut *tx, kb_id).await?;
        Catalogue::new(&classes, &properties, &proposal.draft)?.validate(&proposal)?;
        let mut class_ids: HashMap<String, Uuid> =
            classes.iter().map(|c| (c.key.clone(), c.id)).collect();
        let mut pending: Vec<_> = proposal
            .draft
            .entity_types
            .iter()
            .filter(|c| !class_ids.contains_key(&c.key))
            .collect();
        while !pending.is_empty() {
            let pos = pending
                .iter()
                .position(|c| c.parents.iter().all(|p| class_ids.contains_key(p)))
                .ok_or_else(|| invalid("Unresolved class parents"))?;
            let c = pending.remove(pos);
            let parents: Vec<_> = c.parents.iter().map(|p| class_ids[p]).collect();
            let new_id = crate::ontology::create_entity_type(
                &mut *tx,
                kb_id,
                &c.key,
                &c.label,
                crate::palette::color_for_key(&c.key),
                "circle",
                &parents,
                &c.description,
            )
            .await?;
            class_ids.insert(c.key.clone(), new_id);
        }
        let mut property_ids: HashMap<String, Uuid> =
            properties.iter().map(|p| (p.key.clone(), p.id)).collect();
        for (kind, list) in [
            ("attribute", &proposal.draft.attribute_types),
            ("relation", &proposal.draft.relation_types),
        ] {
            for p in list {
                if property_ids.contains_key(&p.key) {
                    continue;
                }
                let domains: Vec<_> = p.domains.iter().map(|c| class_ids[c]).collect();
                let ranges: Vec<_> = p.ranges.iter().map(|c| class_ids[c]).collect();
                let new_id = crate::ontology::create_relation_type(
                    &mut *tx,
                    kb_id,
                    &p.key,
                    &p.label,
                    "state",
                    RelationAxioms::default(),
                    &p.description,
                    kind,
                    &domains,
                    &ranges,
                    p.datatype.as_deref(),
                    p.unit.as_deref(),
                )
                .await?;
                property_ids.insert(p.key.clone(), new_id);
            }
        }
        let alignment_id = Uuid::now_v7();
        sqlx::query("INSERT INTO table_alignments(id,kb_id,data_source_id,table_name,class_id,proposal_id) VALUES($1,$2,$3,$4,$5,$6)")
            .bind(alignment_id).bind(kb_id).bind(proposal.source_id).bind(&proposal.draft.table)
            .bind(class_ids[&proposal.draft.class]).bind(id).execute(&mut *tx).await?;
        let identities: HashMap<Uuid, Uuid> = proposal
            .attribute_ids
            .iter()
            .map(|(column, id)| {
                let key = &proposal
                    .draft
                    .columns
                    .iter()
                    .find(|c| &c.column == column)
                    .unwrap()
                    .property;
                (*id, property_ids[key])
            })
            .collect();
        for c in &proposal.draft.columns {
            let mut tree = c.expression.clone();
            let mut inputs = BTreeMap::new();
            if let Some(tree) = &mut tree {
                let mut reads = Vec::new();
                Expr::from_json(tree)
                    .map_err(|e| AppError::invalid(e.code(), e.message()))?
                    .predicates(&mut reads);
                for input in reads {
                    let column = proposal
                        .attribute_ids
                        .iter()
                        .find(|(_, id)| **id == input)
                        .unwrap()
                        .0;
                    inputs.insert(identities[&input].to_string(), column.clone());
                }
                remap(tree, &identities)?;
            }
            sqlx::query("INSERT INTO table_alignment_columns(alignment_id,column_name,class_id,property_id,target_class_id,expression,input_columns) VALUES($1,$2,$3,$4,$5,$6,$7)")
                .bind(alignment_id).bind(&c.column).bind(class_ids[&c.class]).bind(property_ids[&c.property])
                .bind(c.target_class.as_ref().map(|c| class_ids[c])).bind(tree).bind(json!(inputs)).execute(&mut *tx).await?;
        }
        // Queue in the same commit: a newly admitted class/property must become
        // visible to the existing ontology index and alignment workers.
        for kind in ["align_types", "align_phrases"] {
            sqlx::query("INSERT INTO jobs(kind,payload) VALUES($1,$2)")
                .bind(kind)
                .bind(json!({"kb_id":kb_id}))
                .execute(&mut *tx)
                .await?;
        }
    }
    sqlx::query(
        "UPDATE ontology_proposals SET status=$2,decided_by=$3,decided_at=now() WHERE id=$1",
    )
    .bind(id)
    .bind(if adopt { "adopted" } else { "rejected" })
    .bind(actor)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

fn remap(value: &mut Value, identities: &HashMap<Uuid, Uuid>) -> AppResult<()> {
    // Only expression nodes are visited. CASE literals and text may themselves
    // spell UUIDs, and are data rather than references.
    if let Some(object) = value.as_object_mut() {
        if let Some(attr) = object.get_mut("attr") {
            let id: Uuid =
                serde_json::from_value(attr.clone()).map_err(|e| invalid(e.to_string()))?;
            *attr = json!(identities
                .get(&id)
                .ok_or_else(|| invalid("Missing expression input"))?);
        }
        for field in ["l", "r", "expr", "case"] {
            if let Some(child) = object.get_mut(field) {
                remap(child, identities)?;
            }
        }
    }
    Ok(())
}
