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

#[derive(Deserialize)]
pub struct Create {
    pub statement_id: Uuid,
    pub relation_type_id: Uuid,
    pub direction: String,
}

pub async fn create(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(kb): Path<Uuid>,
    Json(req): Json<Create>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    require_kb(&state.pool, &user, kb, Role::Editor).await?;
    let id = cases::add(
        &state.pool,
        kb,
        cases::NewCase {
            statement_id: req.statement_id,
            expected_property_id: req.relation_type_id,
            expected_direction: &req.direction,
            created_by: user.id,
            origin: "person",
        },
    )
    .await?;
    Ok((StatusCode::CREATED, Json(json!({"id": id}))))
}

#[cfg(test)]
#[path = "ontology_regression_routes_tests.rs"]
mod tests;
