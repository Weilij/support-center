//! Scheduled report execution (CRD 4658-4661): finds due active definitions,
//! generates a report over a last-24-hours window with a date-stamped title,
//! records execution attempts, advances next-run, and deactivates definitions
//! that exhaust their retry ceiling.

use serde_json::json;

use crate::db::now_iso;
use crate::state::AppState;

use super::handlers::{next_run, GENERATABLE, GENERATABLE_FORMATS};

pub async fn run_due(state: &AppState) -> usize {
    type DueRow = (String, String, Option<String>, Option<String>, Option<String>, i64, i64);
    let due: Vec<DueRow> = sqlx::query_as(
        "SELECT id, name, report_type, format, schedule_type, max_retries, run_count
         FROM scheduled_reports
         WHERE deleted_at IS NULL AND is_active = 1 AND next_run_at <= ?",
    )
    .bind(now_iso())
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    let mut processed = 0;
    for (id, name, kind, format, frequency, max_retries, _) in &due {
        let run_id = uuid::Uuid::new_v4().to_string();
        let started = std::time::Instant::now();
        let _ = sqlx::query(
            "INSERT INTO scheduled_report_runs (id, schedule_id, started_at, status) VALUES (?, ?, ?, 'running')",
        )
        .bind(&run_id)
        .bind(id)
        .bind(now_iso())
        .execute(&state.db)
        .await;

        let kind = kind.as_deref().unwrap_or("conversation_summary");
        let format = format.as_deref().unwrap_or("json");
        let outcome: Result<String, String> = if GENERATABLE.contains(&kind)
            && GENERATABLE_FORMATS.contains(&format)
        {
            // Date-stamped title over a last-24h window (CRD 4660).
            let report_id = uuid::Uuid::new_v4().to_string();
            let title = format!("{name} — {}", chrono::Utc::now().format("%Y-%m-%d"));
            let content = json!({"dataset": kind, "window": "last_24_hours", "generatedAt": now_iso()})
                .to_string();
            let key = format!("reports/{report_id}.{format}");
            match crate::domain::files::store::put_object(
                &state.config.upload_dir, &key, content.as_bytes(),
            )
            .await
            {
                Ok(()) => {
                    let creator: String = sqlx::query_scalar(
                        "SELECT created_by FROM scheduled_reports WHERE id = ?",
                    )
                    .bind(id)
                    .fetch_one(&state.db)
                    .await
                    .unwrap_or_default();
                    let _ = sqlx::query(
                        "INSERT INTO reports (id, title, report_type, format, status, created_by,
                                              time_range, output_url, output_size, completed_at, created_at)
                         VALUES (?, ?, ?, ?, 'completed', ?, 'last_24_hours', ?, ?, ?, ?)",
                    )
                    .bind(&report_id)
                    .bind(&title)
                    .bind(kind)
                    .bind(format)
                    .bind(&creator)
                    .bind(&key)
                    .bind(content.len() as i64)
                    .bind(now_iso())
                    .bind(now_iso())
                    .execute(&state.db)
                    .await;
                    Ok(report_id)
                }
                Err(e) => Err(e.to_string()),
            }
        } else {
            Err(format!("type '{kind}' / format '{format}' is not generatable"))
        };

        match outcome {
            Ok(report_id) => {
                let _ = sqlx::query(
                    "UPDATE scheduled_report_runs SET status = 'success', completed_at = ?, duration_ms = ?, report_id = ? WHERE id = ?",
                )
                .bind(now_iso())
                .bind(started.elapsed().as_millis() as i64)
                .bind(&report_id)
                .bind(&run_id)
                .execute(&state.db)
                .await;
                let _ = sqlx::query(
                    "UPDATE scheduled_reports SET next_run_at = ?, last_run_at = ?, last_status = 'success', run_count = run_count + 1 WHERE id = ?",
                )
                .bind(next_run(frequency.as_deref().unwrap_or("daily")))
                .bind(now_iso())
                .bind(id)
                .execute(&state.db)
                .await;
            }
            Err(error) => {
                let _ = sqlx::query(
                    "UPDATE scheduled_report_runs SET status = 'failed', completed_at = ?, error_message = ? WHERE id = ?",
                )
                .bind(now_iso())
                .bind(&error)
                .bind(&run_id)
                .execute(&state.db)
                .await;
                let failures: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM scheduled_report_runs WHERE schedule_id = ? AND status = 'failed'",
                )
                .bind(id)
                .fetch_one(&state.db)
                .await
                .unwrap_or(0);
                if failures >= *max_retries {
                    // Retry ceiling exhausted: deactivate (CRD 4660).
                    let _ = sqlx::query(
                        "UPDATE scheduled_reports SET is_active = 0, last_status = 'failed' WHERE id = ?",
                    )
                    .bind(id)
                    .execute(&state.db)
                    .await;
                } else {
                    let retry_at = (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339();
                    let _ = sqlx::query(
                        "UPDATE scheduled_reports SET next_run_at = ?, last_status = 'failed' WHERE id = ?",
                    )
                    .bind(retry_at)
                    .bind(id)
                    .execute(&state.db)
                    .await;
                }
            }
        }
        processed += 1;
    }
    processed
}
