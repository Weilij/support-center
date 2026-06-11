//! Undo of reversible actions (CRD §3.5, lines 2543-2583): authorization, restore
//! window, conflict detection, the guarded one-time claim, and the atomic reversal
//! batch that also appends the restore audit entry.

use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::Utc;
use serde_json::{json, Map, Value};
use sqlx::sqlite::Sqlite;
use sqlx::Transaction;
use std::sync::Arc;

use crate::error::AppError;
use crate::state::AppState;

use super::store;

type Result<T = Response> = std::result::Result<T, AppError>;

/// Default restore window when the capture site did not specify one (CRD 2577).
const DEFAULT_WINDOW_HOURS: i64 = 24;

/// Machine-readable restore error (CRD 2563-2574), same body shape as `AppError`.
fn coded(status: StatusCode, code: &'static str, message: &str, data: Option<Value>) -> Response {
    let mut body = json!({
        "success": false,
        "error": message,
        "code": code,
        "timestamp": crate::db::now_iso(),
        "requestId": crate::envelope::request_id(),
    });
    if let Some(d) = data {
        body["data"] = d;
    }
    (status, Json(body)).into_response()
}

fn batch_failed() -> Response {
    coded(
        StatusCode::INTERNAL_SERVER_ERROR,
        "BATCH_FAILED",
        "Failed to apply the restore batch",
        None,
    )
}

// ------------------------------------------------------------------- caller resolution

struct Caller {
    id: String,
    name: String,
    role: String,
}

/// Reads the caller from the session, or — outside production — from the test-only
/// `x-test-user` header carrying a JSON object with id and role (CRD 2548).
async fn resolve_caller(state: &Arc<AppState>, headers: &HeaderMap) -> Option<Caller> {
    if !state.config.is_production() {
        if let Some(raw) = headers.get("x-test-user").and_then(|v| v.to_str().ok()) {
            let v: Value = serde_json::from_str(raw).ok()?;
            let id = v.get("id").and_then(Value::as_str)?.to_string();
            let agent =
                crate::domain::auth::store::find_agent_by_id(&state.db, &id).await.ok().flatten();
            let name =
                agent.as_ref().map(|a| a.display_name.clone()).unwrap_or_else(|| id.clone());
            let role = v
                .get("role")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| agent.as_ref().map(|a| a.role.clone()))
                .unwrap_or_else(|| "agent".into());
            return Some(Caller { id, name, role });
        }
    }
    crate::middleware::auth::authenticate(state, headers)
        .await
        .ok()
        .map(|u| Caller { id: u.id, name: u.display_name, role: u.role })
}

// --------------------------------------------------------------------- restore strategies

#[derive(Debug, Clone, Copy, PartialEq)]
enum Strategy {
    /// Revert recorded prior field values (covers updates, transfers, and undeletes —
    /// the prior snapshot of a deletion carries `deletedAt: null`).
    RevertFields,
    /// Soft-delete a record to undo its creation.
    SoftDelete,
    /// Remove a previously added tag association.
    RemoveTagAssoc,
    /// Re-add a previously removed tag association (tolerates the association being absent).
    AddTagAssoc,
    /// Reinstate a removed team membership (tolerates the membership being absent).
    ReinstateMembership,
}

/// Maps each reversible action code to its undo strategy (CRD 2592-2593); unknown
/// codes surface as RESTORE_HANDLER_NOT_FOUND.
fn strategy_for(action: &str) -> Option<Strategy> {
    match action {
        "tag assign" => Some(Strategy::RemoveTagAssoc),
        "tag unassign" => Some(Strategy::AddTagAssoc),
        "team remove member" => Some(Strategy::ReinstateMembership),
        "create_user" => Some(Strategy::SoftDelete),
        "update_profile" => Some(Strategy::RevertFields),
        "conversation transfer" | "conversation assign" | "conversation unassign"
        | "conversation close" | "conversation reopen" => Some(Strategy::RevertFields),
        a if a.ends_with(" create") => Some(Strategy::SoftDelete),
        a if a.ends_with(" update") || a.ends_with(" delete") => Some(Strategy::RevertFields),
        _ => None,
    }
}

// -------------------------------------------------------- resource allowlist (CRD 2593)

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Text,
    Int,
    Bool,
}

struct FieldSpec {
    key: &'static str,
    column: &'static str,
    kind: Kind,
}

struct TableSpec {
    table: &'static str,
    text_id: bool,
    fields: &'static [FieldSpec],
}

impl TableSpec {
    fn field(&self, key: &str) -> Option<&FieldSpec> {
        self.fields.iter().find(|f| f.key == key)
    }
}

static TAGS: TableSpec = TableSpec {
    table: "tags",
    text_id: false,
    fields: &[
        FieldSpec { key: "name", column: "name", kind: Kind::Text },
        FieldSpec { key: "color", column: "color", kind: Kind::Text },
        FieldSpec { key: "description", column: "description", kind: Kind::Text },
        FieldSpec { key: "isActive", column: "is_active", kind: Kind::Bool },
        FieldSpec { key: "deletedAt", column: "deleted_at", kind: Kind::Text },
    ],
};

static TEAMS: TableSpec = TableSpec {
    table: "teams",
    text_id: false,
    fields: &[
        FieldSpec { key: "name", column: "name", kind: Kind::Text },
        FieldSpec { key: "description", column: "description", kind: Kind::Text },
        FieldSpec { key: "isActive", column: "is_active", kind: Kind::Bool },
        FieldSpec { key: "qrCode", column: "qr_code", kind: Kind::Text },
        FieldSpec { key: "deletedAt", column: "deleted_at", kind: Kind::Text },
    ],
};

static CUSTOMERS: TableSpec = TableSpec {
    table: "customers",
    text_id: false,
    fields: &[
        FieldSpec { key: "displayName", column: "display_name", kind: Kind::Text },
        FieldSpec { key: "email", column: "email", kind: Kind::Text },
        FieldSpec { key: "phone", column: "phone", kind: Kind::Text },
        FieldSpec { key: "avatarUrl", column: "avatar_url", kind: Kind::Text },
        FieldSpec { key: "deletedAt", column: "deleted_at", kind: Kind::Text },
    ],
};

static CONVERSATIONS: TableSpec = TableSpec {
    table: "conversations",
    text_id: true,
    fields: &[
        FieldSpec { key: "teamId", column: "team_id", kind: Kind::Int },
        FieldSpec { key: "status", column: "status", kind: Kind::Text },
        FieldSpec { key: "priority", column: "priority", kind: Kind::Text },
        FieldSpec { key: "deletedAt", column: "deleted_at", kind: Kind::Text },
    ],
};

static AGENTS: TableSpec = TableSpec {
    table: "agents",
    text_id: true,
    fields: &[
        FieldSpec { key: "displayName", column: "display_name", kind: Kind::Text },
        FieldSpec { key: "role", column: "role", kind: Kind::Text },
        FieldSpec { key: "isActive", column: "is_active", kind: Kind::Bool },
        FieldSpec { key: "deletedAt", column: "deleted_at", kind: Kind::Text },
    ],
};

fn table_for(rtype: &str) -> Option<&'static TableSpec> {
    match rtype {
        "tag" => Some(&TAGS),
        "team" => Some(&TEAMS),
        "customer" => Some(&CUSTOMERS),
        "conversation" => Some(&CONVERSATIONS),
        "agent" | "user" => Some(&AGENTS),
        _ => None,
    }
}

/// Restore audit action codes (CRD 2590: the dedicated restore family).
fn restore_action_for(rtype: &str) -> &'static str {
    match rtype {
        "conversation" => "conversation restore",
        "customer" => "customer restore",
        "tag" | "customer_tag" => "tag restore",
        "team" => "team restore",
        "team_member" => "team member restore",
        "delayed_message" => "delayed message restore",
        "agent" | "user" => "user restore",
        _ => "resource restore",
    }
}

// ---------------------------------------------------------------- plan & conflict report

#[derive(Debug, Clone)]
enum IdBind {
    I(i64),
    S(String),
}

enum PlanKind {
    Fields { spec: &'static TableSpec, id: IdBind, old: Value, soft_delete: bool },
    RemoveAssoc { customer_id: i64, tag_id: i64 },
    AddAssoc { customer_id: i64, tag_id: i64, assigned_by: Option<String>, assigned_at: Option<String>, exists: bool },
    Membership { agent_id: String, team_id: i64, role: String, is_primary: bool, joined_at: Option<String>, exists: bool },
}

struct Plan {
    kind: PlanKind,
    conflicts: Vec<Value>,
}

fn conflict(field: &str, original: &Value, current: &Value, restore: &Value) -> Value {
    json!({
        "field": field,
        "originalValue": original,
        "currentValue": current,
        "restoreValue": restore,
    })
}

/// Reads the tracked fields of a live or soft-deleted row as a normalized JSON map.
async fn current_state(
    db: &sqlx::SqlitePool,
    spec: &'static TableSpec,
    id: &IdBind,
) -> sqlx::Result<Option<Map<String, Value>>> {
    let cols = spec
        .fields
        .iter()
        .map(|f| format!("'{}', {}", f.key, f.column))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!("SELECT json_object({cols}) FROM {} WHERE id = ?", spec.table);
    let mut q = sqlx::query_scalar::<_, String>(&sql);
    q = match id {
        IdBind::I(v) => q.bind(*v),
        IdBind::S(s) => q.bind(s.clone()),
    };
    let raw = q.fetch_optional(db).await?;
    Ok(raw
        .and_then(|r| serde_json::from_str::<Value>(&r).ok())
        .and_then(|v| v.as_object().cloned())
        .map(|mut m| {
            // SQLite stores booleans as 0/1; normalize so they compare against snapshots.
            for f in spec.fields.iter().filter(|f| f.kind == Kind::Bool) {
                if let Some(n) = m.get(f.key).and_then(Value::as_i64) {
                    m.insert(f.key.into(), json!(n != 0));
                }
            }
            m
        }))
}

/// Per-field drift: current state vs the state recorded right after the original
/// action; the restore value comes from the prior snapshot (CRD 2594-2595).
fn detect_field_conflicts(
    spec: &'static TableSpec,
    current: &Map<String, Value>,
    old: &Value,
    new: &Value,
) -> Vec<Value> {
    let Some(new_obj) = new.as_object() else { return Vec::new() };
    let mut out = Vec::new();
    for (key, recorded) in new_obj {
        if spec.field(key).is_none() {
            continue; // unknown fields surface as BATCH_FAILED at apply time
        }
        let cur = current.get(key).cloned().unwrap_or(Value::Null);
        if &cur != recorded {
            let restore = old.get(key).cloned().unwrap_or(Value::Null);
            out.push(conflict(key, recorded, &cur, &restore));
        }
    }
    out
}

/// Resolves the target resource and builds the reversal plan, or a coded rejection.
async fn build_plan(
    state: &AppState,
    strategy: Strategy,
    rtype: &str,
    resource_id: &str,
    details: &Value,
) -> Result<std::result::Result<Plan, Response>> {
    let unsupported = || {
        coded(
            StatusCode::UNPROCESSABLE_ENTITY,
            "UNSUPPORTED_RESOURCE_TYPE",
            &format!("Resource type '{rtype}' is not supported for restore"),
            None,
        )
    };
    let missing_resource = || {
        coded(
            StatusCode::UNPROCESSABLE_ENTITY,
            "RESOURCE_NOT_FOUND",
            "Target resource no longer exists",
            None,
        )
    };

    match strategy {
        Strategy::RevertFields | Strategy::SoftDelete => {
            let Some(spec) = table_for(rtype) else {
                return Ok(Err(unsupported()));
            };
            let id = if spec.text_id {
                IdBind::S(resource_id.to_string())
            } else {
                match resource_id.parse::<i64>() {
                    Ok(v) => IdBind::I(v),
                    Err(_) => return Ok(Err(missing_resource())),
                }
            };
            let Some(current) = current_state(&state.db, spec, &id).await? else {
                return Ok(Err(missing_resource()));
            };
            let old = details.get("old").cloned().unwrap_or(Value::Null);
            let new = details.get("new").cloned().unwrap_or(Value::Null);
            if strategy == Strategy::RevertFields {
                // The prior snapshot is the value set being re-applied: it must exist,
                // be non-empty, and stay within the allowlist (else BATCH_FAILED, CRD 2593).
                let valid = old
                    .as_object()
                    .filter(|o| !o.is_empty())
                    .is_some_and(|o| o.keys().all(|k| spec.field(k).is_some()));
                if !valid {
                    return Ok(Err(batch_failed()));
                }
            }
            let conflicts = detect_field_conflicts(spec, &current, &old, &new);
            Ok(Ok(Plan {
                kind: PlanKind::Fields {
                    spec,
                    id,
                    old,
                    soft_delete: strategy == Strategy::SoftDelete,
                },
                conflicts,
            }))
        }
        Strategy::RemoveTagAssoc | Strategy::AddTagAssoc => {
            if rtype != "customer_tag" {
                return Ok(Err(unsupported()));
            }
            let (Some(customer_id), Some(tag_id)) = (
                details.get("customerId").and_then(Value::as_i64),
                details.get("tagId").and_then(Value::as_i64),
            ) else {
                // Required prior-state values missing at capture time (CRD 2593).
                return Ok(Err(batch_failed()));
            };
            let existing: Option<(Option<String>, String)> = sqlx::query_as(
                "SELECT assigned_by, created_at FROM customer_tags WHERE customer_id = ? AND tag_id = ?",
            )
            .bind(customer_id)
            .bind(tag_id)
            .fetch_optional(&state.db)
            .await?;

            if strategy == Strategy::RemoveTagAssoc {
                if existing.is_none() {
                    return Ok(Err(missing_resource()));
                }
                Ok(Ok(Plan {
                    kind: PlanKind::RemoveAssoc { customer_id, tag_id },
                    conflicts: Vec::new(),
                }))
            } else {
                // Add-back tolerates a missing association; an association that
                // reappeared since the unassign is drift (CRD 2557 step 4-5).
                let restore = json!({
                    "assignedBy": details.get("assignedBy").cloned().unwrap_or(Value::Null),
                    "assignedAt": details.get("assignedAt").cloned().unwrap_or(Value::Null),
                });
                let conflicts = match &existing {
                    Some((by, at)) => vec![conflict(
                        "association",
                        &Value::Null,
                        &json!({ "assignedBy": by, "assignedAt": at }),
                        &restore,
                    )],
                    None => Vec::new(),
                };
                Ok(Ok(Plan {
                    kind: PlanKind::AddAssoc {
                        customer_id,
                        tag_id,
                        assigned_by: details
                            .get("assignedBy")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        assigned_at: details
                            .get("assignedAt")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        exists: existing.is_some(),
                    },
                    conflicts,
                }))
            }
        }
        Strategy::ReinstateMembership => {
            if rtype != "team_member" {
                return Ok(Err(unsupported()));
            }
            let old = details.get("old").cloned().unwrap_or(Value::Null);
            let team_id = old.get("teamId").and_then(Value::as_i64);
            let role = old.get("roleInTeam").and_then(Value::as_str);
            let agent_id = old
                .get("agentId")
                .and_then(Value::as_str)
                .unwrap_or(resource_id)
                .to_string();
            let (Some(team_id), Some(role)) = (team_id, role) else {
                return Ok(Err(batch_failed()));
            };
            // A membership-style restore permits a missing current state (CRD 2557 step 4).
            let existing: Option<(String, i64)> = sqlx::query_as(
                "SELECT role, is_primary FROM team_members WHERE agent_id = ? AND team_id = ?",
            )
            .bind(&agent_id)
            .bind(team_id)
            .fetch_optional(&state.db)
            .await?;
            let conflicts = match &existing {
                Some((cur_role, cur_primary)) => vec![conflict(
                    "membership",
                    details.get("new").unwrap_or(&Value::Null),
                    &json!({ "roleInTeam": cur_role, "isPrimary": *cur_primary != 0 }),
                    &old,
                )],
                None => Vec::new(),
            };
            Ok(Ok(Plan {
                kind: PlanKind::Membership {
                    agent_id,
                    team_id,
                    role: role.to_string(),
                    is_primary: old.get("isPrimary").and_then(Value::as_bool).unwrap_or(false),
                    joined_at: old.get("joinedAt").and_then(Value::as_str).map(str::to_string),
                    exists: existing.is_some(),
                },
                conflicts,
            }))
        }
    }
}

// ------------------------------------------------------------------ reversal application

fn proto_err(msg: &str) -> sqlx::Error {
    sqlx::Error::Protocol(msg.to_string())
}

enum BindVal {
    Null,
    I(i64),
    S(String),
}

fn to_bind(kind: Kind, v: &Value) -> sqlx::Result<BindVal> {
    match (kind, v) {
        (_, Value::Null) => Ok(BindVal::Null),
        (Kind::Bool, Value::Bool(b)) => Ok(BindVal::I(*b as i64)),
        (Kind::Bool, Value::Number(n)) if n.as_i64().is_some() => {
            Ok(BindVal::I((n.as_i64().unwrap() != 0) as i64))
        }
        (Kind::Int, Value::Number(n)) if n.as_i64().is_some() => {
            Ok(BindVal::I(n.as_i64().unwrap()))
        }
        (Kind::Text, Value::String(s)) => Ok(BindVal::S(s.clone())),
        _ => Err(proto_err("restore value outside the field allowlist")),
    }
}

async fn apply_plan(
    tx: &mut Transaction<'_, Sqlite>,
    plan: &Plan,
    now: &str,
) -> sqlx::Result<()> {
    match &plan.kind {
        PlanKind::Fields { spec, id, old, soft_delete } => {
            let (mut sets, mut binds): (Vec<String>, Vec<BindVal>) = (Vec::new(), Vec::new());
            if *soft_delete {
                sets.push("deleted_at = ?".into());
                binds.push(BindVal::S(now.to_string()));
                if spec.table == "tags" {
                    sets.push("is_active = 0".into());
                }
            } else {
                let obj = old
                    .as_object()
                    .filter(|o| !o.is_empty())
                    .ok_or_else(|| proto_err("missing prior-state snapshot"))?;
                for (key, v) in obj {
                    let f = spec
                        .field(key)
                        .ok_or_else(|| proto_err("field outside the restore allowlist"))?;
                    sets.push(format!("{} = ?", f.column));
                    binds.push(to_bind(f.kind, v)?);
                }
            }
            sets.push("updated_at = ?".into());
            let sql =
                format!("UPDATE {} SET {} WHERE id = ?", spec.table, sets.join(", "));
            let mut q = sqlx::query(&sql);
            for b in &binds {
                q = match b {
                    BindVal::Null => q.bind(Option::<String>::None),
                    BindVal::I(v) => q.bind(*v),
                    BindVal::S(s) => q.bind(s.clone()),
                };
            }
            q = q.bind(now);
            q = match id {
                IdBind::I(v) => q.bind(*v),
                IdBind::S(s) => q.bind(s.clone()),
            };
            let affected = q.execute(&mut **tx).await?.rows_affected();
            if affected == 0 {
                return Err(proto_err("target resource vanished during restore"));
            }
        }
        PlanKind::RemoveAssoc { customer_id, tag_id } => {
            let affected =
                sqlx::query("DELETE FROM customer_tags WHERE customer_id = ? AND tag_id = ?")
                    .bind(customer_id)
                    .bind(tag_id)
                    .execute(&mut **tx)
                    .await?
                    .rows_affected();
            if affected == 0 {
                return Err(proto_err("tag association vanished during restore"));
            }
        }
        PlanKind::AddAssoc { customer_id, tag_id, assigned_by, assigned_at, exists } => {
            if !exists {
                sqlx::query(
                    "INSERT OR IGNORE INTO customer_tags (customer_id, tag_id, assigned_by, created_at)
                     VALUES (?, ?, ?, ?)",
                )
                .bind(customer_id)
                .bind(tag_id)
                .bind(assigned_by)
                .bind(assigned_at.as_deref().unwrap_or(now))
                .execute(&mut **tx)
                .await?;
            }
        }
        PlanKind::Membership { agent_id, team_id, role, is_primary, joined_at, exists } => {
            if *is_primary {
                sqlx::query("UPDATE team_members SET is_primary = 0 WHERE agent_id = ?")
                    .bind(agent_id)
                    .execute(&mut **tx)
                    .await?;
            }
            if *exists {
                sqlx::query(
                    "UPDATE team_members SET role = ?, is_primary = ? WHERE agent_id = ? AND team_id = ?",
                )
                .bind(role)
                .bind(*is_primary as i64)
                .bind(agent_id)
                .bind(team_id)
                .execute(&mut **tx)
                .await?;
            } else {
                sqlx::query(
                    "INSERT INTO team_members (agent_id, team_id, role, is_primary, joined_at)
                     VALUES (?, ?, ?, ?, ?)",
                )
                .bind(agent_id)
                .bind(team_id)
                .bind(role)
                .bind(*is_primary as i64)
                .bind(joined_at.as_deref().unwrap_or(now))
                .execute(&mut **tx)
                .await?;
            }
        }
    }
    Ok(())
}

// ------------------------------------------------------------------------ HTTP operation

/// POST /api/activities/{id}/restore (CRD 2543-2583).
pub async fn restore_activity(
    State(state): State<Arc<AppState>>,
    Path(raw_id): Path<String>,
    headers: HeaderMap,
    body: std::result::Result<Json<Value>, JsonRejection>,
) -> Result {
    // 1. Validate id, load the entry, confirm reversibility and link state (CRD 2553).
    let id: i64 = raw_id
        .parse()
        .ok()
        .filter(|v| *v > 0)
        .ok_or_else(|| AppError::BadRequest("Invalid activity id".into()))?;
    // A missing or unparseable body is force = false (CRD 2546).
    let force = body
        .ok()
        .and_then(|Json(v)| v.get("force").and_then(Value::as_bool))
        .unwrap_or(false);

    let entry = store::find(&state.db, id)
        .await?
        .ok_or_else(|| AppError::NotFound("Activity not found".into()))?;
    let details = entry.details_json();
    if details.get("reversible").and_then(Value::as_bool) != Some(true) {
        return Ok(coded(
            StatusCode::UNPROCESSABLE_ENTITY,
            "NOT_REVERSIBLE",
            "Activity is not reversible",
            None,
        ));
    }
    match entry.restore_state.as_deref() {
        Some("in_progress") => {
            return Ok(coded(
                StatusCode::CONFLICT,
                "RESTORE_IN_PROGRESS",
                "A restore is already in progress for this activity",
                Some(json!({ "retryAfter": 5 })),
            ))
        }
        Some("restored") => {
            return Ok(coded(
                StatusCode::CONFLICT,
                "ALREADY_RESTORED",
                "Activity has already been restored",
                Some(json!({ "restoredBy": entry.restored_by_log_id })),
            ))
        }
        _ => {}
    }

    // 2. Authenticate and authorize: admin always; the original actor only when the
    //    entry's restore policy does not require admin (CRD 2550-2551).
    let caller = resolve_caller(&state, &headers)
        .await
        .ok_or_else(|| AppError::Unauthorized("Unauthenticated".into()))?;
    let policy = details.get("restorePolicy").cloned().unwrap_or(Value::Null);
    let requires_admin = policy.get("requiresAdmin").and_then(Value::as_bool).unwrap_or(false);
    let is_admin = caller.role == "admin";
    if !(is_admin || (caller.id == entry.agent_id && !requires_admin)) {
        return Err(AppError::Forbidden("Forbidden".into()));
    }

    // 3. The restore window must still be open (CRD 2556, default 24h, CRD 2577).
    let expires = policy
        .get("expiresAt")
        .and_then(Value::as_str)
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&Utc))
        .or_else(|| {
            chrono::DateTime::parse_from_rfc3339(&entry.created_at)
                .ok()
                .map(|d| d.with_timezone(&Utc) + chrono::Duration::hours(DEFAULT_WINDOW_HOURS))
        });
    if expires.is_none_or(|e| Utc::now() > e) {
        return Ok(coded(
            StatusCode::GONE,
            "RESTORE_EXPIRED",
            "The restore window for this activity has elapsed",
            None,
        ));
    }

    // 4. Resource identifier, restore strategy, and supported resource type (CRD 2552).
    let Some(resource_id) = entry.resource_id.clone() else {
        return Ok(coded(
            StatusCode::UNPROCESSABLE_ENTITY,
            "MISSING_RESOURCE_ID",
            "Activity has no associated resource id",
            None,
        ));
    };
    let Some(strategy) = strategy_for(&entry.action) else {
        return Ok(coded(
            StatusCode::UNPROCESSABLE_ENTITY,
            "RESTORE_HANDLER_NOT_FOUND",
            &format!("No restore handler registered for action '{}'", entry.action),
            None,
        ));
    };
    let rtype = entry.resource_type.clone().unwrap_or_default();

    // 5. Resolve current state and run conflict detection (CRD 2557 steps 4-5).
    let plan = match build_plan(&state, strategy, &rtype, &resource_id, &details).await? {
        Ok(p) => p,
        Err(resp) => return Ok(resp),
    };
    if !plan.conflicts.is_empty() && !force {
        return Ok(coded(
            StatusCode::CONFLICT,
            "RESTORE_CONFLICT",
            "Resource has been modified since the original action",
            Some(json!({ "conflicts": plan.conflicts })),
        ));
    }

    // 6. Atomically claim the restore: a guarded one-time transition so concurrent
    //    attempts cannot both proceed (CRD 2557 step 6, 2576).
    let claimed = sqlx::query(
        "UPDATE activity_logs SET restore_state = 'in_progress' WHERE id = ? AND restore_state IS NULL",
    )
    .bind(id)
    .execute(&state.db)
    .await?
    .rows_affected();
    if claimed == 0 {
        let winner = store::find(&state.db, id).await?;
        return Ok(match winner.as_ref().and_then(|w| w.restore_state.as_deref()) {
            Some("restored") => coded(
                StatusCode::CONFLICT,
                "ALREADY_RESTORED",
                "Activity has already been restored",
                Some(json!({ "restoredBy": winner.and_then(|w| w.restored_by_log_id) })),
            ),
            _ => coded(
                StatusCode::CONFLICT,
                "RESTORE_IN_PROGRESS",
                "A restore is already in progress for this activity",
                Some(json!({ "retryAfter": 5 })),
            ),
        });
    }

    // 7-8. Apply the reversal and append the restore audit entry in one atomic batch,
    //      then link the original entry to it (CRD 2557 steps 7-8, 2575-2576).
    let now = crate::db::now_iso();
    let restore_details = json!({
        "reversible": false,
        "restoredActivityId": id,
        "sourceAction": entry.action,
        "force": force,
    });
    let batch: sqlx::Result<i64> = async {
        let mut tx = state.db.begin().await?;
        apply_plan(&mut tx, &plan, &now).await?;
        let new_id = sqlx::query(
            "INSERT INTO activity_logs (agent_id, agent_name, agent_role, action, resource_type, resource_id, details, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&caller.id)
        .bind(&caller.name)
        .bind(&caller.role)
        .bind(restore_action_for(&rtype))
        .bind(&rtype)
        .bind(&resource_id)
        .bind(restore_details.to_string())
        .bind(&now)
        .execute(&mut *tx)
        .await?
        .last_insert_rowid();
        let marked = sqlx::query(
            "UPDATE activity_logs SET restore_state = 'restored', restored_by_log_id = ?, restored_at = ?
             WHERE id = ? AND restore_state = 'in_progress'",
        )
        .bind(new_id)
        .bind(&now)
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if marked != 1 {
            return Err(proto_err("lost restore claim"));
        }
        tx.commit().await?;
        Ok(new_id)
    }
    .await;

    let new_id = match batch {
        Ok(v) => v,
        Err(e) => {
            tracing::error!(error = %e, activity = id, "restore batch failed");
            // Release the claim so a later retry is possible (CRD 2573).
            let _ = sqlx::query(
                "UPDATE activity_logs SET restore_state = NULL WHERE id = ? AND restore_state = 'in_progress'",
            )
            .bind(id)
            .execute(&state.db)
            .await;
            return Ok(batch_failed());
        }
    };

    // Membership writes must invalidate the actor's cached team set.
    if let PlanKind::Membership { agent_id, .. } = &plan.kind {
        state.team_cache.invalidate(agent_id);
    }

    // 9. Realtime: broadcast `resource.restored` through the routed delivery
    //    path; best-effort only — the hub broadcast never fails the
    //    already-committed restore (CRD 2604-2608).
    state.realtime.global(
        "resource.restored",
        json!({
            "resourceType": rtype,
            "resourceId": resource_id,
            "restoredBy": caller.id,
            "restoreActivityId": new_id,
        }),
    );

    Ok(crate::envelope::ok_msg(
        json!({ "restoredActivityId": id, "restoreActivityId": new_id }),
        "Activity restored successfully",
    ))
}
