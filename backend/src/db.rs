use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

pub async fn init_pool(database_url: &str) -> Result<PgPool, sqlx::Error> {
    let pool = PgPoolOptions::new()
        .max_connections(16)
        .connect(database_url)
        .await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    Ok(pool)
}

/// Current time as the canonical ISO-8601 UTC string used in every TEXT timestamp column.
pub fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// Renumber placeholders in dynamically assembled SQL: any existing `$N` is
/// normalized back to `?`, then every `?` is numbered left-to-right. Correct
/// whenever binds are applied in the textual order of the final SQL (the
/// convention throughout this codebase).
pub fn pg_params(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len() + 8);
    let mut n = 0usize;
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '$' => {
                // swallow an existing number
                while chars.peek().map(|d| d.is_ascii_digit()).unwrap_or(false) {
                    chars.next();
                }
                n += 1;
                out.push_str(&format!("${n}"));
            }
            '?' => {
                n += 1;
                out.push_str(&format!("${n}"));
            }
            _ => out.push(c),
        }
    }
    out
}
