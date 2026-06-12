//! Demo data seeder: creates an initial administrator, a team, and sample
//! customers/conversations so a fresh clone is usable immediately.
//!
//!   cargo run --example seed
//!   # then sign in with admin@example.com / admin123

use mcss_backend::domain::auth::store::hash_password;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = mcss_backend::config::Config::from_env();
    let pool = mcss_backend::db::init_pool(&config.database_url).await?;
    let now = mcss_backend::db::now_iso();

    // Administrator (idempotent: skipped when the email already exists).
    let existing: Option<String> =
        sqlx::query_scalar("SELECT id FROM agents WHERE email = 'admin@example.com' AND deleted_at IS NULL")
            .fetch_optional(&pool)
            .await?;
    if existing.is_some() {
        println!("admin@example.com already exists — nothing to do");
        return Ok(());
    }
    let admin_id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO agents (id, email, password_hash, display_name, role, is_active, created_at)
         VALUES ($1, 'admin@example.com', $2, '系統管理員', 'admin', 1, $3)",
    )
    .bind(&admin_id)
    .bind(hash_password("admin123").map_err(|e| format!("hash: {e}"))?)
    .bind(&now)
    .execute(&pool)
    .await?;

    // Demo team with the admin as primary supervisor.
    let team_id = sqlx::query_scalar::<_, i64>("INSERT INTO teams (name, description, created_at) VALUES ('客服一組', '示範團隊', $1) RETURNING id")
        .bind(&now)
        .fetch_one(&pool)
        .await?
        ;
    sqlx::query(
        "INSERT INTO team_members (agent_id, team_id, role, is_primary, joined_at) VALUES ($1, $2, 'supervisor', 1, $3)",
    )
    .bind(&admin_id)
    .bind(team_id)
    .bind(&now)
    .execute(&pool)
    .await?;

    // Sample customers with open conversations and a few messages.
    for (n, (user, name, text)) in [
        ("U-demo-1", "王小明", "你好，請問營業時間？"),
        ("U-demo-2", "陳美玲", "我的訂單還沒收到"),
    ]
    .iter()
    .enumerate()
    {
        let customer_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO customers (platform, platform_user_id, display_name, source_team_id, created_at)
             VALUES ('line', $1, $2, $3, $4) RETURNING id",
        )
        .bind(user)
        .bind(name)
        .bind(team_id)
        .bind(&now)
        .fetch_one(&pool)
        .await?
        ;
        let conversation_id = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO conversations (id, customer_id, team_id, status, priority, last_message_at, created_at)
             VALUES ($1, $2, $3, 'active', $4, $5, $6)",
        )
        .bind(&conversation_id)
        .bind(customer_id)
        .bind(team_id)
        .bind(if n == 1 { "high" } else { "normal" })
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await?;
        sqlx::query(
            "INSERT INTO messages (id, conversation_id, sender_type, customer_id, content, content_type,
                                   delivery_status, sender_name, created_at)
             VALUES ($1, $2, 'customer', $3, $4, 'text', 'delivered', $5, $6)",
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(&conversation_id)
        .bind(customer_id)
        .bind(text)
        .bind(name)
        .bind(&now)
        .execute(&pool)
        .await?;
    }

    println!("Seeded: admin@example.com / admin123, team '客服一組', 2 demo conversations");
    println!("Database: {}", config.database_url);
    Ok(())
}
