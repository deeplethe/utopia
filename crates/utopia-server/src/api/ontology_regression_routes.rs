//! A case records a person's expected interpretation; alignment supplies its result.
use crate::{auth::AuthUser, error::ApiResult, state::AppState};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use utopia_core::models::Role;
use utopia_store::{access::require_kb, ontology_regressions as cases};
use uuid::Uuid;

#[derive(Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedBinding {
    #[default]
    Bound,
    None,
}

#[derive(Deserialize)]
pub struct Create {
    pub statement_id: Uuid,
    #[serde(default)]
    pub expected_binding: ExpectedBinding,
    pub relation_type_id: Option<Uuid>,
    pub direction: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb): Path<Uuid>,
) -> ApiResult<Json<Value>> {
    require_kb(&state.pool, &user, kb, Role::Viewer).await?;
    Ok(Json(json!({"cases": cases::views(&state.pool, kb).await?})))
}

pub async fn create(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb): Path<Uuid>,
    Json(req): Json<Create>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    require_kb(&state.pool, &user, kb, Role::Editor).await?;
    let valid = match req.expected_binding {
        ExpectedBinding::Bound => req.relation_type_id.is_some() && req.direction.is_some(),
        ExpectedBinding::None => req.relation_type_id.is_none() && req.direction.is_none(),
    };
    if !valid {
        return Err(utopia_core::AppError::invalid(
            "bad_case_expectation",
            "A bound expectation needs a property and direction; a none expectation has neither.",
        )
        .into());
    }
    let id = cases::add(
        &state.pool,
        kb,
        cases::NewCase {
            statement_id: req.statement_id,
            expected_property_id: req.relation_type_id,
            expected_direction: req.direction.as_deref(),
            created_by: user.id,
            origin: "person",
        },
    )
    .await?;
    state.emit_review(kb);
    Ok((StatusCode::CREATED, Json(json!({"id": id}))))
}

#[cfg(test)]
#[path = "ontology_regression_routes_tests.rs"]
mod tests;
