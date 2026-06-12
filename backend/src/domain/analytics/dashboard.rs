//! Dashboards, widgets & realtime dashboard control (CRD 4327-4444).
//! Configurations persist per user in system_settings under
//! `dashboard:{user}:{id}`.

use axum::extract::{Path, Query, State};
use axum::response::Response;
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use std::sync::Arc;

use crate::db::now_iso;
use crate::envelope;
use crate::error::AppError;
use crate::middleware::auth::{is_manager_or_admin, AuthUser};
use crate::state::AppState;

type Result<T = Response> = std::result::Result<T, AppError>;

pub const WIDGET_TYPES: &[&str] = &["metric", "chart", "table", "gauge", "progress", "status"];

fn require_team_level(user: &AuthUser) -> Result<()> {
    if is_manager_or_admin(user) {
        Ok(())
    } else {
        Err(AppError::Forbidden("Administrator or team-level role required".into()))
    }
}

fn config_key(user_id: &str, dashboard_id: &str) -> String {
    format!("dashboard:{user_id}:{dashboard_id}")
}

async fn load_config(state: &AppState, user_id: &str, dashboard_id: &str) -> Option<Value> {
    let stored: Option<String> =
        sqlx::query_scalar("SELECT value FROM system_settings WHERE key = $1")
            .bind(config_key(user_id, dashboard_id))
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten();
    stored.and_then(|s| serde_json::from_str(&s).ok())
}

async fn store_config(state: &AppState, user_id: &str, dashboard_id: &str, config: &Value) {
    let _ = sqlx::query(
        "INSERT INTO system_settings (key, value, updated_at) VALUES ($1, $2, $3)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
    )
    .bind(config_key(user_id, dashboard_id))
    .bind(config.to_string())
    .bind(now_iso())
    .execute(&state.db)
    .await;
}

fn default_config(user: &AuthUser, dashboard_id: &str) -> Value {
    json!({
        "id": dashboard_id,
        "name": "Default dashboard",
        "layout": {"type": "grid", "columns": 12},
        "widgets": [],
        "refreshInterval": 30000,
        "autoRefresh": true,
        "permissions": {"owner": user.id, "viewers": [], "editors": []},
        "createdAt": now_iso(),
        "updatedAt": now_iso(),
        "createdBy": user.id,
    })
}

pub async fn health(Extension(_user): Extension<AuthUser>) -> Result {
    Ok(envelope::ok(json!({
        "status": "healthy",
        "timestamp": now_iso(),
        "services": {"configStore": "healthy", "dataResolver": "healthy"},
    })))
}

pub async fn widget_types(Extension(_user): Extension<AuthUser>) -> Result {
    let types: Vec<Value> = WIDGET_TYPES
        .iter()
        .map(|t| json!({"type": t, "name": t, "description": format!("{t} widget")}))
        .collect();
    Ok(envelope::ok(types))
}

#[derive(Deserialize)]
pub struct TemplateQuery {
    pub category: Option<String>,
    #[serde(rename = "widgetType")]
    pub widget_type: Option<String>,
}

fn dashboard_templates() -> Vec<Value> {
    vec![
        json!({"id": "overview", "name": "Operations overview", "category": "operations",
               "widgets": [
                   {"id": "w-total", "type": "metric", "title": "Total conversations",
                    "dataSource": {"type": "analytics", "query": "total_conversations"},
                    "position": {"x": 0, "y": 0, "width": 4, "height": 2}},
                   {"id": "w-trend", "type": "chart", "title": "Conversation trend",
                    "dataSource": {"type": "analytics", "query": "conversation_trend"},
                    "position": {"x": 4, "y": 0, "width": 8, "height": 4}},
               ]}),
        json!({"id": "team", "name": "Team performance", "category": "team", "widgets": []}),
    ]
}

fn widget_templates() -> Vec<Value> {
    vec![
        json!({"id": "metric-total", "name": "Total metric", "category": "metrics", "widgetType": "metric",
               "widget": {"type": "metric", "title": "Metric",
                          "dataSource": {"type": "analytics", "query": "total_conversations"},
                          "position": {"x": 0, "y": 0, "width": 4, "height": 2}}}),
        json!({"id": "chart-trend", "name": "Trend chart", "category": "charts", "widgetType": "chart",
               "widget": {"type": "chart", "title": "Trend",
                          "dataSource": {"type": "analytics", "query": "conversation_trend"},
                          "position": {"x": 0, "y": 0, "width": 8, "height": 4}}}),
    ]
}

pub async fn list_dashboard_templates(
    Extension(_user): Extension<AuthUser>,
    Query(q): Query<TemplateQuery>,
) -> Result {
    let mut templates = dashboard_templates();
    if let Some(category) = &q.category {
        templates.retain(|t| t["category"] == category.as_str());
    }
    templates.sort_by_key(|t| t["name"].as_str().unwrap_or("").to_string());
    Ok(envelope::ok(templates))
}

pub async fn list_widget_templates(
    Extension(_user): Extension<AuthUser>,
    Query(q): Query<TemplateQuery>,
) -> Result {
    let mut templates = widget_templates();
    if let Some(category) = &q.category {
        templates.retain(|t| t["category"] == category.as_str());
    }
    if let Some(kind) = &q.widget_type {
        templates.retain(|t| t["widgetType"] == kind.as_str());
    }
    Ok(envelope::ok(templates))
}

// ------------------------------------------------ config get/save

pub async fn get_config(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    dashboard_id: Option<Path<String>>,
) -> Result {
    let id = dashboard_id.map(|Path(p)| p).unwrap_or_else(|| "default".into());
    let config = load_config(&state, &user.id, &id)
        .await
        .unwrap_or_else(|| default_config(&user, &id));
    Ok(envelope::ok(config))
}

pub async fn save_config(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    dashboard_id: Option<Path<String>>,
    Json(body): Json<Value>,
) -> Result {
    let name = body.get("name").and_then(Value::as_str).unwrap_or("").trim();
    if name.is_empty() {
        return Err(AppError::Validation(
            "Validation failed".into(),
            vec![crate::error::FieldProblem {
                field: "name".into(),
                message: "name is required".into(),
                value: None,
            }],
        ));
    }
    if let Some(layout) = body.get("layout") {
        let kind = layout.get("type").and_then(Value::as_str).unwrap_or("grid");
        if !["grid", "flex", "absolute", "responsive"].contains(&kind) {
            return Err(AppError::BadRequest("Invalid layout type".into()));
        }
        if let Some(columns) = layout.get("columns").and_then(Value::as_i64) {
            if !(1..=24).contains(&columns) {
                return Err(AppError::BadRequest("layout.columns must be 1-24".into()));
            }
        }
    }
    if let Some(interval) = body.get("refreshInterval").and_then(Value::as_i64) {
        if interval < 5000 {
            return Err(AppError::BadRequest("refreshInterval must be at least 5000 ms".into()));
        }
    }
    // Resolved id: path id, else body id, else default (CRD 4362).
    let id = dashboard_id
        .map(|Path(p)| p)
        .or_else(|| body.get("id").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| "default".into());

    let existing = load_config(&state, &user.id, &id).await;
    let mut config = body;
    let obj = config.as_object_mut().unwrap();
    obj.insert("id".into(), json!(id));
    obj.entry("permissions".to_string()).or_insert_with(|| {
        json!({"owner": user.id, "viewers": [], "editors": []})
    });
    obj.entry("refreshInterval".to_string()).or_insert(json!(30000));
    obj.entry("autoRefresh".to_string()).or_insert(json!(true));
    obj.entry("widgets".to_string()).or_insert(json!([]));
    obj.insert(
        "createdAt".into(),
        existing
            .as_ref()
            .and_then(|e| e.get("createdAt").cloned())
            .unwrap_or_else(|| json!(now_iso())),
    );
    obj.insert("updatedAt".into(), json!(now_iso()));
    obj.insert("createdBy".into(), json!(user.id));
    store_config(&state, &user.id, &id, &config).await;
    Ok(envelope::ok_msg(config, "Dashboard saved"))
}

// ------------------------------------------------ widget data resolution

async fn resolve_widget_data(state: &AppState, widget: &Value) -> Value {
    let query = widget
        .pointer("/dataSource/query")
        .and_then(Value::as_str)
        .unwrap_or("");
    let data = match query {
        "total_conversations" => {
            let count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM conversations WHERE deleted_at IS NULL",
            )
            .fetch_one(&state.db)
            .await
            .unwrap_or(0);
            json!({"value": count})
        }
        "total_messages" => {
            let count: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE deleted_at IS NULL")
                    .fetch_one(&state.db)
                    .await
                    .unwrap_or(0);
            json!({"value": count})
        }
        "conversation_trend" => {
            let rows: Vec<(String, i64)> = sqlx::query_as(
                "SELECT substr(created_at, 1, 10), COUNT(*) FROM conversations
                 WHERE deleted_at IS NULL GROUP BY 1 ORDER BY 1 DESC LIMIT 14",
            )
            .fetch_all(&state.db)
            .await
            .unwrap_or_default();
            json!(rows.iter().rev().map(|(d, c)| json!({"timestamp": d, "value": c})).collect::<Vec<_>>())
        }
        _ => json!(null),
    };
    json!({
        "widgetId": widget.get("id"),
        "type": widget.get("type"),
        "data": data,
        "loading": false,
        "lastUpdate": now_iso(),
        "metadata": {},
    })
}

#[derive(Deserialize)]
pub struct DataQuery {
    #[serde(rename = "dashboardId")]
    pub dashboard_id: Option<String>,
    #[serde(rename = "timeRange")]
    pub time_range: Option<String>,
}

fn parse_time_range(raw: &Option<String>) -> Result<()> {
    if let Some(raw) = raw {
        if serde_json::from_str::<Value>(raw).is_err() {
            return Err(AppError::BadRequest("Invalid timeRange parameter".into()));
        }
    }
    Ok(())
}

pub async fn dashboard_data(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    dashboard_id: Option<Path<String>>,
    Query(q): Query<DataQuery>,
) -> Result {
    parse_time_range(&q.time_range)?;
    let id = dashboard_id.map(|Path(p)| p).unwrap_or_else(|| "default".into());
    let config = load_config(&state, &user.id, &id)
        .await
        .unwrap_or_else(|| default_config(&user, &id));
    let mut data = Map::new();
    for widget in config["widgets"].as_array().into_iter().flatten() {
        if let Some(wid) = widget.get("id").and_then(Value::as_str) {
            data.insert(wid.to_string(), resolve_widget_data(&state, widget).await);
        }
    }
    Ok(envelope::ok(Value::Object(data)))
}

pub async fn widget_data(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(widget_id): Path<String>,
    Query(q): Query<DataQuery>,
) -> Result {
    parse_time_range(&q.time_range)?;
    let id = q.dashboard_id.clone().unwrap_or_else(|| "default".into());
    let config = load_config(&state, &user.id, &id)
        .await
        .unwrap_or_else(|| default_config(&user, &id));
    let widget = config["widgets"]
        .as_array()
        .and_then(|w| w.iter().find(|x| x["id"] == widget_id.as_str()))
        .ok_or_else(|| AppError::NotFound("Widget not found".into()))?;
    Ok(envelope::ok(resolve_widget_data(&state, widget).await))
}

// ------------------------------------------------ widget management

#[derive(Deserialize, Default)]
pub struct CloneBody {
    #[serde(rename = "newWidgetId")]
    pub new_widget_id: Option<String>,
    #[serde(rename = "dashboardId")]
    pub dashboard_id: Option<String>,
}

pub async fn clone_widget(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(widget_id): Path<String>,
    body: Option<Json<CloneBody>>,
) -> Result {
    require_team_level(&user)?;
    let body = body.map(|Json(b)| b).unwrap_or_default();
    let id = body.dashboard_id.clone().unwrap_or_else(|| "default".into());
    let mut config = load_config(&state, &user.id, &id)
        .await
        .ok_or_else(|| AppError::NotFound("Widget not found".into()))?;
    let source = config["widgets"]
        .as_array()
        .and_then(|w| w.iter().find(|x| x["id"] == widget_id.as_str()))
        .cloned()
        .ok_or_else(|| AppError::NotFound("Widget not found".into()))?;
    let mut clone = source;
    clone["id"] = json!(body
        .new_widget_id
        .unwrap_or_else(|| format!("{widget_id}-copy-{}", uuid::Uuid::new_v4())));
    config["widgets"].as_array_mut().unwrap().push(clone.clone());
    config["updatedAt"] = json!(now_iso());
    store_config(&state, &user.id, &id, &config).await;
    Ok(envelope::ok_msg(clone, "Widget cloned"))
}

pub async fn create_from_template(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(template_id): Path<String>,
    body: Option<Json<Value>>,
) -> Result {
    let template = dashboard_templates()
        .into_iter()
        .find(|t| t["id"] == template_id.as_str())
        .ok_or_else(|| AppError::NotFound("Template not found".into()))?;
    let overrides = body.map(|Json(b)| b).unwrap_or(json!({}));
    let id = format!("dash-{}", uuid::Uuid::new_v4());
    let config = json!({
        "id": id,
        "name": overrides.get("name").and_then(Value::as_str).unwrap_or(template["name"].as_str().unwrap_or("Dashboard")),
        "layout": {"type": "grid", "columns": 12},
        "widgets": template["widgets"],
        "refreshInterval": overrides.get("refreshInterval").and_then(Value::as_i64).unwrap_or(30000).max(5000),
        "autoRefresh": overrides.get("autoRefresh").and_then(Value::as_bool).unwrap_or(true),
        "theme": overrides.get("theme"),
        "permissions": {"owner": user.id, "viewers": [], "editors": []},
        "createdAt": now_iso(),
        "updatedAt": now_iso(),
        "createdBy": user.id,
    });
    store_config(&state, &user.id, &id, &config).await;
    Ok(envelope::ok_msg(config, "Dashboard created from template"))
}

pub async fn create_widget_from_template(
    State(_state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(template_id): Path<String>,
    body: Option<Json<Value>>,
) -> Result {
    require_team_level(&user)?;
    let template = widget_templates()
        .into_iter()
        .find(|t| t["id"] == template_id.as_str())
        .ok_or_else(|| AppError::NotFound("Widget template not found".into()))?;
    let overrides = body.map(|Json(b)| b).unwrap_or(json!({}));
    let mut widget = template["widget"].clone();
    widget["id"] = json!(format!("w-{}", uuid::Uuid::new_v4()));
    if let Some(title) = overrides.get("title") {
        widget["title"] = title.clone();
    }
    if let Some(position) = overrides.get("position") {
        widget["position"] = position.clone();
    }
    if let Some(ds) = overrides.get("dataSource") {
        widget["dataSource"] = ds.clone();
    }
    Ok(envelope::ok_msg(widget, "Widget created from template"))
}

fn validate_widget(body: &Value, partial: bool) -> Result<()> {
    if !partial {
        let kind = body.get("type").and_then(Value::as_str).unwrap_or("");
        if !WIDGET_TYPES.contains(&kind) {
            return Err(AppError::BadRequest(format!(
                "Widget type must be one of {WIDGET_TYPES:?}"
            )));
        }
        if body.get("title").and_then(Value::as_str).unwrap_or("").trim().is_empty() {
            return Err(AppError::BadRequest("Widget title is required".into()));
        }
        let position = body.get("position").ok_or_else(|| {
            AppError::BadRequest("Widget position is required".into())
        })?;
        let w = position.get("width").and_then(Value::as_i64).unwrap_or(0);
        let h = position.get("height").and_then(Value::as_i64).unwrap_or(0);
        if w < 1 || h < 1 {
            return Err(AppError::BadRequest("Widget width/height must be >= 1".into()));
        }
    }
    if let Some(interval) = body.get("refreshInterval").and_then(Value::as_i64) {
        if interval < 5000 {
            return Err(AppError::BadRequest("refreshInterval must be at least 5000 ms".into()));
        }
    }
    Ok(())
}

pub async fn create_widget(
    Extension(user): Extension<AuthUser>,
    Json(body): Json<Value>,
) -> Result {
    require_team_level(&user)?;
    validate_widget(&body, false)?;
    let mut widget = body;
    widget["id"] = json!(format!("w-{}", uuid::Uuid::new_v4()));
    Ok(envelope::ok_msg(widget, "Widget created"))
}

pub async fn update_widget(
    Extension(user): Extension<AuthUser>,
    Path(widget_id): Path<String>,
    Json(body): Json<Value>,
) -> Result {
    require_team_level(&user)?;
    validate_widget(&body, true)?;
    let mut widget = body;
    widget["id"] = json!(widget_id);
    Ok(envelope::ok_msg(widget, "Widget updated"))
}

#[derive(Deserialize, Default)]
pub struct OptimizeBody {
    #[serde(rename = "dashboardId")]
    pub dashboard_id: Option<String>,
    #[serde(rename = "containerWidth")]
    pub container_width: Option<i64>,
}

pub async fn optimize_layout(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    body: Option<Json<OptimizeBody>>,
) -> Result {
    require_team_level(&user)?;
    let body = body.map(|Json(b)| b).unwrap_or_default();
    let id = body.dashboard_id.clone().unwrap_or_else(|| "default".into());
    let width = body.container_width.unwrap_or(12).clamp(1, 24);
    let mut config = load_config(&state, &user.id, &id)
        .await
        .unwrap_or_else(|| default_config(&user, &id));
    // Reflow widgets left-to-right within the container width.
    let (mut x, mut y, mut row_h) = (0i64, 0i64, 0i64);
    if let Some(widgets) = config["widgets"].as_array_mut() {
        for widget in widgets {
            let w = widget["position"]["width"].as_i64().unwrap_or(4).min(width);
            let h = widget["position"]["height"].as_i64().unwrap_or(2);
            if x + w > width {
                x = 0;
                y += row_h;
                row_h = 0;
            }
            widget["position"]["x"] = json!(x);
            widget["position"]["y"] = json!(y);
            x += w;
            row_h = row_h.max(h);
        }
    }
    config["updatedAt"] = json!(now_iso());
    store_config(&state, &user.id, &id, &config).await;
    Ok(envelope::ok_msg(config, "Layout optimized"))
}

// ------------------------------------------------ realtime dashboard control

#[derive(Deserialize)]
pub struct BroadcastBody {
    #[serde(rename = "dashboardId")]
    pub dashboard_id: Option<String>,
    #[serde(rename = "widgetId")]
    pub widget_id: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub data: Option<Value>,
}

pub async fn broadcast(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Json(body): Json<BroadcastBody>,
) -> Result {
    require_team_level(&user)?;
    let dashboard = body
        .dashboard_id
        .clone()
        .filter(|d| !d.is_empty())
        .ok_or_else(|| AppError::BadRequest("dashboardId is required".into()))?;
    match body.kind.as_deref() {
        Some("widget_update") => {
            let widget = body
                .widget_id
                .clone()
                .filter(|w| !w.is_empty())
                .ok_or_else(|| AppError::BadRequest("widgetId is required for widget_update".into()))?;
            state.realtime.global(
                "dashboard_widget_update",
                json!({"dashboardId": dashboard, "widgetId": widget, "data": body.data}),
            );
            Ok(envelope::ok_msg(json!({"broadcast": true}), "widget_update broadcast dispatched"))
        }
        Some("config_change") => {
            state.realtime.global(
                "dashboard_config_change",
                json!({"dashboardId": dashboard, "config": body.data}),
            );
            Ok(envelope::ok_msg(json!({"broadcast": true}), "config_change broadcast dispatched"))
        }
        other => Err(AppError::BadRequest(format!(
            "Unsupported broadcast type '{}'",
            other.unwrap_or("")
        ))),
    }
}

pub async fn trigger_widget(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path((dashboard_id, widget_id)): Path<(String, String)>,
) -> Result {
    let config = load_config(&state, &user.id, &dashboard_id)
        .await
        .unwrap_or_else(|| default_config(&user, &dashboard_id));
    let widget = config["widgets"]
        .as_array()
        .and_then(|w| w.iter().find(|x| x["id"] == widget_id.as_str()))
        .ok_or_else(|| AppError::NotFound("Widget not found".into()))?;
    let data = resolve_widget_data(&state, widget).await;
    state.realtime.global(
        "dashboard_widget_update",
        json!({"dashboardId": dashboard_id, "widgetId": widget_id, "data": data}),
    );
    Ok(envelope::ok_msg(data, "Widget refreshed"))
}

pub async fn trigger_dashboard(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
    Path(dashboard_id): Path<String>,
) -> Result {
    let config = load_config(&state, &user.id, &dashboard_id)
        .await
        .unwrap_or_else(|| default_config(&user, &dashboard_id));
    let mut data = Map::new();
    let mut updated = Vec::new();
    for widget in config["widgets"].as_array().into_iter().flatten() {
        if let Some(wid) = widget.get("id").and_then(Value::as_str) {
            let resolved = resolve_widget_data(&state, widget).await;
            state.realtime.global(
                "dashboard_widget_update",
                json!({"dashboardId": dashboard_id, "widgetId": wid, "data": resolved}),
            );
            data.insert(wid.to_string(), resolved);
            updated.push(wid.to_string());
        }
    }
    Ok(envelope::ok_msg(
        json!({"data": data, "updatedWidgets": updated}),
        "Dashboard refreshed",
    ))
}

pub async fn realtime_status(
    State(state): State<Arc<AppState>>,
    Extension(user): Extension<AuthUser>,
) -> Result {
    if !user.is_admin() {
        return Err(AppError::Forbidden("Administrator role required".into()));
    }
    let total = state.realtime.connection_count();
    Ok(envelope::ok(json!({
        "status": {
            "totalConnections": total,
            "connectionsByDashboard": {},
            "connectionsByUser": {},
        },
        "timestamp": now_iso(),
    })))
}

pub async fn realtime_health(
    State(state): State<Arc<AppState>>,
    Extension(_user): Extension<AuthUser>,
) -> Result {
    let total = state.realtime.connection_count();
    Ok(envelope::ok(json!({
        "status": "healthy",
        "timestamp": now_iso(),
        "service": "dashboard-realtime",
        "totalConnections": total,
        "metrics": {"totalConnections": total, "dashboards": 0, "users": 0},
    })))
}

pub async fn realtime_cleanup(
    Extension(user): Extension<AuthUser>,
) -> Result {
    if !user.is_admin() {
        return Err(AppError::Forbidden("Administrator role required".into()));
    }
    Ok(envelope::ok_msg(json!({"cleaned": 0}), "Expired connections cleaned"))
}
