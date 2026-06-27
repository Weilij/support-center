//! Auto-reply management endpoints (CRD §2.5, lines 1341-1410).

use axum::extract::{Path, Query, State};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use std::sync::Arc;

use crate::db::now_iso;
use crate::envelope;
use crate::error::{AppError, HandlerResult as Result};
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

/// id, team_id, name, trigger, priority, active, fallback, created_by, created, updated, deleted
type RuleRow = (
    i64,
    Option<i64>,
    String,
    String,
    i64,
    i64,
    i64,
    Option<String>,
    String,
    Option<String>,
    Option<String>,
);
/// id, team_id, day_of_week, start, end, timezone, active
type ScheduleRow = (
    i64,
    i64,
    i64,
    Option<String>,
    Option<String>,
    Option<String>,
    i64,
);
/// id, rule_id, rule_name, conversation, customer, trigger, response, matched, platform, method, created
type LogRow = (
    i64,
    Option<i64>,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
);

const TRIGGER_TYPES: &[&str] = &["welcome", "greeting", "keyword", "off_hours", "fallback"];
const CONDITION_TYPES: &[&str] = &["exact", "contains", "regex", "message_type"];
const ACTION_TYPES: &[&str] = &["text", "image", "flex"];

#[derive(Deserialize)]
pub struct ScopeQuery {
    pub scope: Option<String>,
    #[serde(rename = "teamId")]
    pub team_id: Option<i64>,
    pub page: Option<i64>,
    #[serde(rename = "pageSize")]
    pub page_size: Option<i64>,
    #[serde(rename = "ruleId")]
    pub rule_id: Option<String>,
    pub platform: Option<String>,
    #[serde(rename = "dateFrom")]
    pub date_from: Option<String>,
}

/// Team resolution chain: explicit teamId -> request-context team -> caller's
/// primary team (CRD 1344).
fn resolve_team(q: &ScopeQuery, user: &AuthUser) -> Option<i64> {
    q.team_id.or(user.context_team_id).or(user.primary_team_id)
}

fn page_params(q: &ScopeQuery) -> (i64, i64) {
    let page = q.page.unwrap_or(1).max(1);
    let size = q.page_size.unwrap_or(50).clamp(1, 100);
    (page, size)
}

async fn rule_view(db: &PgPool, rule_id: i64) -> Result<Value> {
    let row: Option<RuleRow> = sqlx::query_as(
        "SELECT id, team_id, name, trigger_type, priority, is_active, allow_fallback,
                    created_by, created_at, updated_at, deleted_at
             FROM auto_reply_rules WHERE id = $1",
    )
    .bind(rule_id)
    .fetch_optional(db)
    .await?;
    let Some((
        id,
        team_id,
        name,
        trigger,
        priority,
        active,
        fallback,
        created_by,
        created,
        updated,
        deleted,
    )) = row
    else {
        return Err(AppError::NotFound("Rule not found".into()));
    };
    let conditions: Vec<(i64, String, Option<String>, i64, String)> = sqlx::query_as(
        "SELECT id, condition_type, value, case_sensitive, match_mode
         FROM auto_reply_conditions WHERE rule_id = $1 ORDER BY id",
    )
    .bind(id)
    .fetch_all(db)
    .await?;
    let actions: Vec<(i64, String, Option<String>, i64)> = sqlx::query_as(
        "SELECT id, action_type, content, sort_order FROM auto_reply_actions
         WHERE rule_id = $1 ORDER BY sort_order, id",
    )
    .bind(id)
    .fetch_all(db)
    .await?;
    Ok(json!({
        "id": id,
        "teamId": team_id,
        "name": name,
        "triggerType": trigger,
        "priority": priority,
        "isActive": active != 0,
        "allowPushFallback": fallback != 0,
        "createdBy": created_by,
        "createdAt": created,
        "updatedAt": updated,
        "deletedAt": deleted,
        "conditions": conditions.iter().map(|(cid, t, v, cs, m)| json!({
            "id": cid, "conditionType": t, "value": v, "caseSensitive": *cs != 0, "matchMode": m,
        })).collect::<Vec<_>>(),
        "actions": actions.iter().map(|(aid, t, c, o)| json!({
            "id": aid, "actionType": t, "content": c, "sortOrder": o,
        })).collect::<Vec<_>>(),
    }))
}

// ---------------------------------------------------------------- rules CRUD

pub async fn list_rules(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<ScopeQuery>,
) -> Result {
    let global = q.scope.as_deref() == Some("global");
    let team = if global {
        None
    } else {
        Some(resolve_team(&q, &user).ok_or_else(|| {
            AppError::BadRequest("teamId is required (or use scope=global)".into())
        })?)
    };
    let (page, size) = page_params(&q);
    let total: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM auto_reply_rules
         WHERE deleted_at IS NULL AND (($1 IS NULL AND team_id IS NULL) OR team_id = $2)",
    )
    .bind(team)
    .bind(team)
    .fetch_one(&state.db)
    .await?;
    let ids: Vec<(i64,)> = sqlx::query_as(
        "SELECT id FROM auto_reply_rules
         WHERE deleted_at IS NULL AND (($1 IS NULL AND team_id IS NULL) OR team_id = $2)
         ORDER BY priority ASC, id ASC LIMIT $3 OFFSET $4",
    )
    .bind(team)
    .bind(team)
    .bind(size)
    .bind((page - 1) * size)
    .fetch_all(&state.db)
    .await?;
    let mut items = Vec::with_capacity(ids.len());
    for (id,) in ids {
        items.push(rule_view(&state.db, id).await?);
    }
    Ok(envelope::paginated(&items, page, size, total))
}

#[derive(Deserialize)]
pub struct RuleBody {
    pub name: Option<String>,
    #[serde(rename = "triggerType")]
    pub trigger_type: Option<String>,
    pub priority: Option<i64>,
    #[serde(rename = "isActive")]
    pub is_active: Option<bool>,
    #[serde(rename = "allowPushFallback")]
    pub allow_push_fallback: Option<bool>,
    pub conditions: Option<Vec<Value>>,
    pub actions: Option<Vec<Value>>,
}

fn validate_conditions(conditions: &[Value]) -> Result<()> {
    for c in conditions {
        let t = c
            .get("conditionType")
            .or_else(|| c.get("type"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if !CONDITION_TYPES.contains(&t) {
            return Err(AppError::BadRequest(format!(
                "Invalid condition type '{t}': must be one of {CONDITION_TYPES:?}"
            )));
        }
    }
    Ok(())
}

fn validate_actions(actions: &[Value]) -> Result<()> {
    for a in actions {
        let t = a
            .get("actionType")
            .or_else(|| a.get("type"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if !ACTION_TYPES.contains(&t) {
            return Err(AppError::BadRequest(format!(
                "Invalid action type '{t}': must be one of {ACTION_TYPES:?}"
            )));
        }
    }
    Ok(())
}

async fn insert_conditions(db: &PgPool, rule_id: i64, conditions: &[Value]) -> Result<()> {
    for c in conditions {
        sqlx::query(
            "INSERT INTO auto_reply_conditions (rule_id, condition_type, value, case_sensitive, match_mode)
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(rule_id)
        .bind(c.get("conditionType").or_else(|| c.get("type")).and_then(Value::as_str).unwrap_or("contains"))
        .bind(c.get("value").and_then(Value::as_str).unwrap_or(""))
        .bind(c.get("caseSensitive").and_then(Value::as_bool).unwrap_or(false) as i64)
        .bind(c.get("matchMode").and_then(Value::as_str).unwrap_or("any"))
        .execute(db)
        .await?;
    }
    Ok(())
}

async fn insert_actions(db: &PgPool, rule_id: i64, actions: &[Value]) -> Result<()> {
    for (idx, a) in actions.iter().enumerate() {
        let content = match a.get("content") {
            Some(Value::String(s)) => s.clone(),
            Some(v) => v.to_string(),
            None => String::new(),
        };
        sqlx::query(
            "INSERT INTO auto_reply_actions (rule_id, action_type, content, sort_order)
             VALUES ($1, $2, $3, $4)",
        )
        .bind(rule_id)
        .bind(
            a.get("actionType")
                .or_else(|| a.get("type"))
                .and_then(Value::as_str)
                .unwrap_or("text"),
        )
        .bind(content)
        .bind(
            a.get("sortOrder")
                .and_then(Value::as_i64)
                .unwrap_or(idx as i64),
        )
        .execute(db)
        .await?;
    }
    Ok(())
}

pub async fn create_rule(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<ScopeQuery>,
    Json(body): Json<RuleBody>,
) -> Result {
    let global = q.scope.as_deref() == Some("global");
    let team = if global {
        None
    } else {
        Some(resolve_team(&q, &user).ok_or_else(|| {
            AppError::BadRequest("teamId is required (or use scope=global)".into())
        })?)
    };
    let name = body.name.as_deref().unwrap_or("").trim().to_string();
    if name.is_empty() {
        return Err(AppError::BadRequest("name is required".into()));
    }
    let trigger = body.trigger_type.as_deref().unwrap_or("");
    if !TRIGGER_TYPES.contains(&trigger) {
        return Err(AppError::BadRequest(format!(
            "Invalid trigger type '{trigger}': must be one of {TRIGGER_TYPES:?}"
        )));
    }
    if let Some(conditions) = &body.conditions {
        validate_conditions(conditions)?;
    }
    if let Some(actions) = &body.actions {
        validate_actions(actions)?;
    }

    let now = now_iso();
    let rule_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO auto_reply_rules
            (team_id, name, trigger_type, priority, is_active, allow_fallback, created_by, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING id",
    )
    .bind(team)
    .bind(&name)
    .bind(trigger)
    .bind(body.priority.unwrap_or(100))
    .bind(body.is_active.unwrap_or(true) as i64)
    .bind(body.allow_push_fallback.unwrap_or(false) as i64)
    .bind(&user.id)
    .bind(&now)
    .fetch_one(&state.db)
    .await?
    ;

    if let Some(conditions) = &body.conditions {
        insert_conditions(&state.db, rule_id, conditions).await?;
    }
    if let Some(actions) = &body.actions {
        insert_actions(&state.db, rule_id, actions).await?;
    }
    state.auto_reply_cache.invalidate(team);

    let view = rule_view(&state.db, rule_id).await?;
    Ok(envelope::created(view))
}

pub async fn update_rule(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Path(id): Path<String>,
    Json(body): Json<RuleBody>,
) -> Result {
    let rule_id: i64 = id
        .parse()
        .map_err(|_| AppError::BadRequest("Invalid rule ID".into()))?;
    let existing: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT team_id FROM auto_reply_rules WHERE id = $1 AND deleted_at IS NULL")
            .bind(rule_id)
            .fetch_optional(&state.db)
            .await?;
    let Some((team,)) = existing else {
        return Err(AppError::NotFound("Rule not found".into()));
    };

    if let Some(name) = &body.name {
        sqlx::query("UPDATE auto_reply_rules SET name = $1 WHERE id = $2")
            .bind(name.trim())
            .bind(rule_id)
            .execute(&state.db)
            .await?;
    }
    if let Some(trigger) = &body.trigger_type {
        sqlx::query("UPDATE auto_reply_rules SET trigger_type = $1 WHERE id = $2")
            .bind(trigger)
            .bind(rule_id)
            .execute(&state.db)
            .await?;
    }
    if let Some(priority) = body.priority {
        sqlx::query("UPDATE auto_reply_rules SET priority = $1 WHERE id = $2")
            .bind(priority)
            .bind(rule_id)
            .execute(&state.db)
            .await?;
    }
    if let Some(active) = body.is_active {
        sqlx::query("UPDATE auto_reply_rules SET is_active = $1 WHERE id = $2")
            .bind(active as i64)
            .bind(rule_id)
            .execute(&state.db)
            .await?;
    }
    if let Some(fallback) = body.allow_push_fallback {
        sqlx::query("UPDATE auto_reply_rules SET allow_fallback = $1 WHERE id = $2")
            .bind(fallback as i64)
            .bind(rule_id)
            .execute(&state.db)
            .await?;
    }
    // A supplied array is a wholesale replace, not a merge (CRD 1364/1368).
    if let Some(conditions) = &body.conditions {
        sqlx::query("DELETE FROM auto_reply_conditions WHERE rule_id = $1")
            .bind(rule_id)
            .execute(&state.db)
            .await?;
        insert_conditions(&state.db, rule_id, conditions).await?;
    }
    if let Some(actions) = &body.actions {
        sqlx::query("DELETE FROM auto_reply_actions WHERE rule_id = $1")
            .bind(rule_id)
            .execute(&state.db)
            .await?;
        insert_actions(&state.db, rule_id, actions).await?;
    }
    sqlx::query("UPDATE auto_reply_rules SET updated_at = $1 WHERE id = $2")
        .bind(now_iso())
        .bind(rule_id)
        .execute(&state.db)
        .await?;
    state.auto_reply_cache.invalidate(team);

    let view = rule_view(&state.db, rule_id).await?;
    Ok(envelope::ok(view))
}

pub async fn delete_rule(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
    Path(id): Path<String>,
) -> Result {
    let rule_id: i64 = id
        .parse()
        .map_err(|_| AppError::BadRequest("Invalid rule ID".into()))?;
    let existing: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT team_id FROM auto_reply_rules WHERE id = $1 AND deleted_at IS NULL")
            .bind(rule_id)
            .fetch_optional(&state.db)
            .await?;
    let Some((team,)) = existing else {
        return Err(AppError::NotFound("Rule not found".into()));
    };
    sqlx::query(
        "UPDATE auto_reply_rules SET deleted_at = $1, is_active = 0, updated_at = $2 WHERE id = $3",
    )
    .bind(now_iso())
    .bind(now_iso())
    .bind(rule_id)
    .execute(&state.db)
    .await?;
    state.auto_reply_cache.invalidate(team);
    Ok(envelope::ok(json!({"id": rule_id})))
}

// ---------------------------------------------------------------- schedules

pub async fn get_schedules(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<ScopeQuery>,
) -> Result {
    let team =
        resolve_team(&q, &user).ok_or_else(|| AppError::BadRequest("teamId is required".into()))?;
    let rows: Vec<ScheduleRow> = sqlx::query_as(
        "SELECT id, team_id, day_of_week, start_time, end_time, timezone, is_active
             FROM auto_reply_business_hours WHERE team_id = $1 ORDER BY day_of_week",
    )
    .bind(team)
    .fetch_all(&state.db)
    .await?;
    let items: Vec<Value> = rows
        .iter()
        .map(|(id, t, d, s, e, tz, a)| {
            json!({
                "id": id, "teamId": t, "dayOfWeek": d, "startTime": s, "endTime": e,
                "timezone": tz, "isActive": *a != 0,
            })
        })
        .collect();
    Ok(envelope::ok(items))
}

#[derive(Deserialize)]
pub struct SchedulesBody {
    pub timezone: Option<String>,
    pub schedules: Option<Value>,
}

fn valid_hhmm(s: &str) -> bool {
    let bytes = s.as_bytes();
    if bytes.len() != 5 || bytes[2] != b':' {
        return false;
    }
    let (h, m) = (&s[0..2], &s[3..5]);
    matches!((h.parse::<u8>(), m.parse::<u8>()), (Ok(h), Ok(m)) if h <= 23 && m <= 59)
}

pub async fn replace_schedules(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<ScopeQuery>,
    Json(body): Json<SchedulesBody>,
) -> Result {
    let team =
        resolve_team(&q, &user).ok_or_else(|| AppError::BadRequest("teamId is required".into()))?;
    let entries = body
        .schedules
        .as_ref()
        .and_then(Value::as_array)
        .filter(|a| !a.is_empty())
        .ok_or_else(|| AppError::BadRequest("schedules array is required".into()))?
        .clone();
    let timezone = body.timezone.unwrap_or_else(|| "Asia/Taipei".into());

    for entry in &entries {
        let day = entry.get("dayOfWeek").and_then(Value::as_i64).unwrap_or(-1);
        if !(0..=6).contains(&day) {
            return Err(AppError::BadRequest(format!(
                "Invalid dayOfWeek '{day}': must be 0-6"
            )));
        }
        for key in ["startTime", "endTime"] {
            let v = entry.get(key).and_then(Value::as_str).unwrap_or("");
            if !valid_hhmm(v) {
                return Err(AppError::BadRequest(format!(
                    "Invalid {key} '{v}': must be 24-hour HH:mm"
                )));
            }
        }
    }

    // Wholesale replace per team (CRD 1392/1396).
    sqlx::query("DELETE FROM auto_reply_business_hours WHERE team_id = $1")
        .bind(team)
        .execute(&state.db)
        .await?;
    for entry in &entries {
        sqlx::query(
            "INSERT INTO auto_reply_business_hours
                (team_id, day_of_week, start_time, end_time, timezone, is_active)
             VALUES ($1, $2, $3, $4, $5, $6)",
        )
        .bind(team)
        .bind(entry.get("dayOfWeek").and_then(Value::as_i64))
        .bind(entry.get("startTime").and_then(Value::as_str))
        .bind(entry.get("endTime").and_then(Value::as_str))
        .bind(&timezone)
        .bind(
            entry
                .get("isActive")
                .and_then(Value::as_bool)
                .unwrap_or(true) as i64,
        )
        .execute(&state.db)
        .await?;
    }

    get_schedules(State(state), Extension(user), Query(q)).await
}

// ---------------------------------------------------------------- audit logs

pub async fn list_logs(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<ScopeQuery>,
) -> Result {
    let team =
        resolve_team(&q, &user).ok_or_else(|| AppError::BadRequest("teamId is required".into()))?;
    let rule_filter: Option<i64> = match &q.rule_id {
        None => None,
        Some(raw) => Some(
            raw.parse()
                .map_err(|_| AppError::BadRequest("ruleId must be an integer".into()))?,
        ),
    };
    if let Some(p) = &q.platform {
        if crate::platform::Platform::parse(p).is_none() {
            return Err(AppError::BadRequest(format!(
                "Invalid platform '{p}': must be one of line, facebook, instagram, shopee"
            )));
        }
    }
    if let Some(d) = &q.date_from {
        if chrono::DateTime::parse_from_rfc3339(d).is_err()
            && chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").is_err()
        {
            return Err(AppError::BadRequest(
                "dateFrom must be a valid ISO-8601 date".into(),
            ));
        }
    }
    let (page, size) = page_params(&q);

    // Logs whose rule is owned by the team OR is global (CRD 1402).
    let base_where = "FROM auto_reply_logs l
         LEFT JOIN auto_reply_rules r ON r.id = l.rule_id
         WHERE (r.team_id = $1 OR r.team_id IS NULL)
           AND ($2 IS NULL OR l.rule_id = $3)
           AND ($4 IS NULL OR l.platform = $5)
           AND ($6 IS NULL OR l.created_at >= $7)";
    let total: i64 = sqlx::query_scalar(&crate::db::pg_params(&format!(
        "SELECT COUNT(*) {base_where}"
    )))
    .bind(team)
    .bind(rule_filter)
    .bind(rule_filter)
    .bind(&q.platform)
    .bind(&q.platform)
    .bind(&q.date_from)
    .bind(&q.date_from)
    .fetch_one(&state.db)
    .await?;
    let today_start = chrono::Utc::now().format("%Y-%m-%dT00:00:00").to_string();
    let today_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM auto_reply_logs l
         LEFT JOIN auto_reply_rules r ON r.id = l.rule_id
         WHERE (r.team_id = $1 OR r.team_id IS NULL) AND l.created_at >= $2",
    )
    .bind(team)
    .bind(&today_start)
    .fetch_one(&state.db)
    .await?;

    let rows: Vec<LogRow> =
        sqlx::query_as(&crate::db::pg_params(&format!(
            "SELECT l.id, l.rule_id, r.name, l.conversation_id, l.customer_id, l.trigger_content,
                    l.response_content, l.matched_condition, l.platform, l.delivery_method, l.created_at
             {base_where} ORDER BY l.created_at DESC, l.id DESC LIMIT $1 OFFSET $2"
        )))
        .bind(team)
        .bind(rule_filter)
        .bind(rule_filter)
        .bind(&q.platform)
        .bind(&q.platform)
        .bind(&q.date_from)
        .bind(&q.date_from)
        .bind(size)
        .bind((page - 1) * size)
        .fetch_all(&state.db)
        .await?;

    let items: Vec<Value> = rows
        .iter()
        .map(
            |(
                id,
                rule_id,
                rule_name,
                conv,
                cust,
                trigger,
                response,
                matched,
                platform,
                method,
                created,
            )| {
                json!({
                    "id": id, "ruleId": rule_id, "ruleName": rule_name,
                    "conversationId": conv, "customerId": cust,
                    "triggerContent": trigger, "responseContent": response,
                    "matchedCondition": matched, "platform": platform,
                    "deliveryMethod": method, "createdAt": created,
                })
            },
        )
        .collect();
    let total_pages = if total == 0 {
        0
    } else {
        (total + size - 1) / size
    };
    Ok(envelope::ok(json!({
        "items": items,
        "page": page,
        "pageSize": size,
        "limit": size,
        "total": total,
        "todayCount": today_count,
        "totalPages": total_pages,
        "hasNext": page < total_pages,
        "hasPrev": page > 1 && total_pages > 0,
    })))
}

// ---------------------------------------------------------------- health

pub async fn health(Path(component): Path<String>) -> Result {
    Ok(envelope::ok(json!({
        "status": "healthy",
        "component": format!("auto-reply-{component}"),
        "timestamp": now_iso(),
    })))
}
