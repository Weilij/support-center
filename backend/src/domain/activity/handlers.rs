//! Read, statistics, and cleanup handlers for the audit trail (CRD §3.5, lines 2462-2541).

use axum::extract::{Path, Query, State};
use axum::response::Response;
use axum::Extension;
use chrono::{SecondsFormat, TimeZone, Utc};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::sync::Arc;

use crate::envelope;
use crate::error::{AppError, FieldProblem};
use crate::middleware::auth::AuthUser;
use crate::state::AppState;

use super::store::{self, ListFilter};

type Result<T = Response> = std::result::Result<T, AppError>;

fn validation(field: &str, message: &str) -> AppError {
    AppError::Validation(
        message.to_string(),
        vec![FieldProblem { field: field.into(), message: message.into(), value: None }],
    )
}

/// Admin gate for the statistics/cleanup family (CRD 2497, 2506, 2519, ...): plain 403.
fn require_admin(user: &AuthUser) -> Result<()> {
    if user.is_admin() {
        Ok(())
    } else {
        Err(AppError::Forbidden("Forbidden".into()))
    }
}

fn iso(dt: chrono::DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// Accepts full ISO-8601 timestamps or bare `YYYY-MM-DD` dates (date-only end bounds
/// extend to end-of-day so the named day is included).
fn parse_iso(raw: &str, end_of_day: bool) -> Option<chrono::DateTime<Utc>> {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(raw) {
        return Some(dt.with_timezone(&Utc));
    }
    let date = chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d").ok()?;
    let t = if end_of_day {
        date.and_hms_milli_opt(23, 59, 59, 999)?
    } else {
        date.and_hms_opt(0, 0, 0)?
    };
    Some(Utc.from_utc_datetime(&t))
}

/// Lenient day-window parameter: invalid or non-positive values fall back to the default.
fn lenient_days(raw: &Option<String>, default: i64) -> i64 {
    raw.as_deref()
        .and_then(|d| d.trim().parse::<i64>().ok())
        .filter(|d| *d > 0)
        .unwrap_or(default)
}

/// Trailing window of `days` ending now, as a filter plus its ISO bounds.
fn window(days: i64) -> (ListFilter, String, String) {
    let end = Utc::now();
    let start = end - chrono::Duration::days(days);
    let (s, e) = (iso(start), iso(end));
    (ListFilter { start: Some(s.clone()), end: Some(e.clone()), ..Default::default() }, s, e)
}

// ------------------------------------------------- List activity entries (CRD 2462-2473)

#[derive(Deserialize)]
pub struct ListQuery {
    pub page: Option<String>,
    pub limit: Option<String>,
    #[serde(rename = "pageSize")]
    pub page_size: Option<String>,
    #[serde(rename = "userId")]
    pub user_id: Option<String>,
    pub action: Option<String>,
    #[serde(rename = "resourceType")]
    pub resource_type: Option<String>,
    #[serde(rename = "startDate")]
    pub start_date: Option<String>,
    #[serde(rename = "endDate")]
    pub end_date: Option<String>,
}

/// Page/page-size values must be integers in 1..=1000 when supplied (CRD 2472).
fn parse_bounded(raw: &Option<String>, field: &str) -> Result<Option<i64>> {
    match raw.as_deref().map(str::trim) {
        None | Some("") => Ok(None),
        Some(s) => {
            let v: i64 = s
                .parse()
                .map_err(|_| validation(field, &format!("{field} must be an integer between 1 and 1000")))?;
            if !(1..=1000).contains(&v) {
                return Err(validation(field, &format!("{field} must be between 1 and 1000")));
            }
            Ok(Some(v))
        }
    }
}

pub async fn list_activities(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<ListQuery>,
) -> Result {
    let page = parse_bounded(&q.page, "page")?.unwrap_or(1);
    let raw_limit = match parse_bounded(&q.limit, "limit")? {
        Some(v) => v,
        None => parse_bounded(&q.page_size, "pageSize")?.unwrap_or(50),
    };
    // Validated up to 1000 but the effective page size is capped at 100 (CRD 2465).
    let limit = raw_limit.min(100);

    let start = match q.start_date.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        None => None,
        Some(s) => Some(
            parse_iso(s, false)
                .ok_or_else(|| validation("startDate", "startDate must be a valid ISO 8601 date"))?,
        ),
    };
    let end = match q.end_date.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        None => None,
        Some(s) => Some(
            parse_iso(s, true)
                .ok_or_else(|| validation("endDate", "endDate must be a valid ISO 8601 date"))?,
        ),
    };
    if let (Some(s), Some(e)) = (start, end) {
        if s > e {
            return Err(validation("startDate", "startDate must be before endDate"));
        }
    }

    // Non-administrators are silently scoped to their own entries (CRD 2467, 2473).
    let user_id = if user.is_admin() {
        q.user_id.clone().filter(|v| !v.trim().is_empty())
    } else {
        Some(user.id.clone())
    };

    let filter = ListFilter {
        user_id,
        action: q.action.clone().filter(|v| !v.trim().is_empty()),
        resource_type: q.resource_type.clone().filter(|v| !v.trim().is_empty()),
        start: start.map(iso),
        end: end.map(iso),
    };
    let (rows, total) = store::list(&state.db, &filter, page, limit).await?;
    let items: Vec<Value> = rows.iter().map(store::entry_view).collect();
    Ok(envelope::paginated(&items, page, limit, total))
}

// --------------------------------------------- Get a single activity entry (CRD 2475-2482)

pub async fn get_activity(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(raw_id): Path<String>,
) -> Result {
    let id: i64 = raw_id
        .parse()
        .ok()
        .filter(|v| *v > 0)
        .ok_or_else(|| AppError::BadRequest("Invalid activity id".into()))?;
    let row = store::find(&state.db, id)
        .await?
        .ok_or_else(|| AppError::NotFound("Activity not found".into()))?;
    // Admins may view any entry; others only entries they themselves performed (CRD 2479).
    if !user.is_admin() && row.agent_id != user.id {
        return Err(AppError::Forbidden("Forbidden".into()));
    }
    Ok(envelope::ok(store::entry_view(&row)))
}

// ------------------------------------------ Per-actor activity statistics (CRD 2484-2493)

#[derive(Deserialize)]
pub struct DaysQuery {
    pub days: Option<String>,
}

pub async fn user_stats(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(target): Path<String>,
    Query(q): Query<DaysQuery>,
) -> Result {
    if !user.is_admin() && target != user.id {
        return Err(AppError::Forbidden("Forbidden".into()));
    }
    let days = lenient_days(&q.days, 30);
    let (mut filter, _, _) = window(days);
    filter.user_id = Some(target);

    let total = store::count(&state.db, &filter).await?;
    let breakdown = store::action_breakdown(&state.db, &filter).await?;
    let (recent, _) = store::list(&state.db, &filter, 1, 10).await?;

    let mut by_action = Map::new();
    for (action, c) in breakdown {
        by_action.insert(action, json!(c));
    }
    Ok(envelope::ok(json!({
        "totalActions": total,
        "actionBreakdown": by_action,
        "recentActivities": recent.iter().map(store::entry_view).collect::<Vec<_>>(),
        "period": { "days": days },
    })))
}

// -------------------------------------------------- Cleanup (purge old) (CRD 2495-2502)

pub async fn cleanup(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<DaysQuery>,
) -> Result {
    require_admin(&user)?;
    // Retention must be an integer within 30..=3650 days; the 30-day floor is a hard
    // guarantee (CRD 2500-2502).
    let days = match q.days.as_deref().map(str::trim) {
        None | Some("") => 90,
        Some(s) => s
            .parse::<i64>()
            .ok()
            .filter(|d| (30..=3650).contains(d))
            .ok_or_else(|| validation("days", "days must be an integer between 30 and 3650"))?,
    };
    let cutoff = iso(Utc::now() - chrono::Duration::days(days));
    let deleted = store::purge_before(&state.db, &cutoff).await?;
    Ok(envelope::ok_msg(
        json!({ "deletedCount": deleted, "retentionDays": days }),
        &format!("Cleaned up {deleted} activity records older than {days} days"),
    ))
}

// ------------------------------------------------- Statistics overview (CRD 2504-2509)

/// Overview-shaped aggregate shared with the custom-period endpoint (CRD 2526-2529).
async fn overview_payload(
    state: &AppState,
    filter: &ListFilter,
    days: i64,
    start: &str,
    end: &str,
) -> Result<Value> {
    let total = store::count(&state.db, filter).await?;
    let mut by_action = Map::new();
    for (action, c) in store::action_breakdown(&state.db, filter).await? {
        by_action.insert(action, json!(c));
    }
    let top = store::top_users(&state.db, filter, 10)
        .await?
        .into_iter()
        .map(|(name, role, c)| json!({ "userName": name, "userRole": role, "count": c }))
        .collect::<Vec<_>>();
    let daily = store::daily_counts(&state.db, filter)
        .await?
        .into_iter()
        .map(|(d, c)| json!({ "date": d, "count": c }))
        .collect::<Vec<_>>();
    Ok(json!({
        "totalActivities": total,
        "actionBreakdown": by_action,
        "topUsers": top,
        "dailyActivities": daily,
        "period": { "days": days, "startDate": start, "endDate": end },
    }))
}

pub async fn overview(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<DaysQuery>,
) -> Result {
    require_admin(&user)?;
    let days = lenient_days(&q.days, 7);
    let (filter, start, end) = window(days);
    Ok(envelope::ok(overview_payload(&state, &filter, days, &start, &end).await?))
}

// -------------------------------------------- Resource-type statistics (CRD 2511-2513)

fn percent(count: i64, total: i64) -> f64 {
    if total == 0 {
        0.0
    } else {
        ((count as f64) * 10000.0 / (total as f64)).round() / 100.0
    }
}

fn resource_label(rtype: &str) -> &str {
    match rtype {
        "conversation" => "会话",
        "message" => "消息",
        "user" | "agent" => "用户",
        "team" => "团队",
        "customer" => "客户",
        "system" => "系统",
        "file" => "文件",
        "qr_code" => "二维码",
        "webhook" => "Webhook",
        "integration" => "集成",
        "tag" => "标签",
        "customer_tag" => "客户标签",
        "team_member" => "团队成员",
        "delayed_message" => "延迟消息",
        other => other,
    }
}

pub async fn resource_stats(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<DaysQuery>,
) -> Result {
    require_admin(&user)?;
    let days = lenient_days(&q.days, 30);
    let (filter, start, end) = window(days);
    let counts = store::resource_counts(&state.db, &filter).await?;
    let total: i64 = counts.iter().map(|(_, c)| c).sum();
    let items = counts
        .iter()
        .map(|(rtype, c)| {
            json!({
                "resourceType": rtype,
                "count": c,
                "percentage": percent(*c, total),
                "label": resource_label(rtype),
            })
        })
        .collect::<Vec<_>>();
    Ok(envelope::ok(json!({
        "resources": items,
        "total": total,
        "period": { "days": days, "startDate": start, "endDate": end },
    })))
}

// ------------------------------------------ Role distribution statistics (CRD 2515-2517)

fn role_label(role: &str) -> &str {
    match role {
        "admin" => "管理员",
        "agent" => "客服",
        other => other,
    }
}

pub async fn role_stats(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<DaysQuery>,
) -> Result {
    require_admin(&user)?;
    let days = lenient_days(&q.days, 30);
    let (filter, start, end) = window(days);
    let counts = store::role_counts(&state.db, &filter).await?;
    let total: i64 = counts.iter().map(|(_, c)| c).sum();
    let items = counts
        .iter()
        .map(|(role, c)| {
            json!({
                "role": role,
                "count": c,
                "percentage": percent(*c, total),
                "label": role_label(role),
            })
        })
        .collect::<Vec<_>>();
    Ok(envelope::ok(json!({
        "roles": items,
        "total": total,
        "period": { "days": days, "startDate": start, "endDate": end },
    })))
}

// --------------------------------------------- Custom-period statistics (CRD 2519-2524)

#[derive(Deserialize)]
pub struct CustomRangeQuery {
    #[serde(rename = "startDate")]
    pub start_date: Option<String>,
    #[serde(rename = "endDate")]
    pub end_date: Option<String>,
}

pub async fn custom_stats(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<CustomRangeQuery>,
) -> Result {
    require_admin(&user)?;
    let raw_start = q.start_date.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let raw_end = q.end_date.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let (Some(raw_start), Some(raw_end)) = (raw_start, raw_end) else {
        return Err(AppError::Validation(
            "Start date and end date are required".into(),
            vec![FieldProblem {
                field: "startDate".into(),
                message: "Start date and end date are required".into(),
                value: None,
            }],
        ));
    };
    let start = parse_iso(raw_start, false)
        .ok_or_else(|| validation("startDate", "startDate must be a valid ISO 8601 date"))?;
    let end = parse_iso(raw_end, true)
        .ok_or_else(|| validation("endDate", "endDate must be a valid ISO 8601 date"))?;
    if start > end {
        return Err(validation("startDate", "startDate must be before endDate"));
    }
    let days = (end - start).num_days().max(1);
    let (s, e) = (iso(start), iso(end));
    let filter = ListFilter { start: Some(s.clone()), end: Some(e.clone()), ..Default::default() };
    Ok(envelope::ok(overview_payload(&state, &filter, days, &s, &e).await?))
}

// --------------------------------------------------------- Activity trends (CRD 2526-2528)

pub async fn trends(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<DaysQuery>,
) -> Result {
    require_admin(&user)?;
    let days = lenient_days(&q.days, 30);
    let (filter, start, end) = window(days);
    let rows = store::daily_action_counts(&state.db, &filter).await?;

    // Fold (day, action, count) into ordered per-day series with action breakdowns.
    let mut order: Vec<String> = Vec::new();
    let mut per_day: std::collections::HashMap<String, (i64, Map<String, Value>)> =
        std::collections::HashMap::new();
    for (day, action, c) in rows {
        let slot = per_day.entry(day.clone()).or_insert_with(|| {
            order.push(day.clone());
            (0, Map::new())
        });
        slot.0 += c;
        slot.1.insert(action, json!(c));
    }
    let series = order
        .iter()
        .map(|day| {
            let (total, actions) = &per_day[day];
            json!({ "date": day, "total": total, "actions": actions })
        })
        .collect::<Vec<_>>();
    Ok(envelope::ok(json!({
        "trends": series,
        "period": { "days": days, "startDate": start, "endDate": end },
    })))
}

// -------------------------------------------------------- Activity heatmap (CRD 2530-2532)

fn intensity(count: i64) -> &'static str {
    if count >= 50 {
        "high"
    } else if count >= 20 {
        "medium"
    } else {
        "low"
    }
}

pub async fn heatmap(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<DaysQuery>,
) -> Result {
    require_admin(&user)?;
    let days = lenient_days(&q.days, 30);
    let (filter, start, end) = window(days);
    let buckets = store::heat_buckets(&state.db, &filter)
        .await?
        .into_iter()
        .map(|(day, hour, c)| {
            json!({ "date": day, "hour": hour, "count": c, "intensity": intensity(c) })
        })
        .collect::<Vec<_>>();
    Ok(envelope::ok(json!({
        "heatmap": buckets,
        "period": { "days": days, "startDate": start, "endDate": end },
    })))
}

// ----------------------------------------------------- Performance metrics (CRD 2534-2536)

pub async fn metrics(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Query(q): Query<DaysQuery>,
) -> Result {
    require_admin(&user)?;
    let days = lenient_days(&q.days, 7);
    let (filter, start, end) = window(days);

    let total = store::count(&state.db, &filter).await?;
    let avg_per_day = ((total as f64) / (days as f64) * 100.0).round() / 100.0;
    let peak_hour = store::hour_counts(&state.db, &filter).await?.first().map(|(h, _)| *h);
    let most_active = store::top_users(&state.db, &filter, 1)
        .await?
        .first()
        .map(|(name, _, _)| name.clone());
    let most_common = store::action_breakdown(&state.db, &filter)
        .await?
        .first()
        .map(|(action, _)| action.clone());
    let load = if avg_per_day > 1000.0 {
        "high"
    } else if avg_per_day > 500.0 {
        "medium"
    } else {
        "low"
    };
    Ok(envelope::ok(json!({
        "avgActivitiesPerDay": avg_per_day,
        "peakHour": peak_hour,
        "mostActiveUser": most_active,
        "mostCommonAction": most_common,
        "systemLoad": load,
        "period": { "days": days, "startDate": start, "endDate": end },
    })))
}
