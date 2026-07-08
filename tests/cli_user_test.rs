use sqlx::PgPool;
use std::env;

#[tokio::test]
#[ignore]
async fn test_user_create_and_list() {
    let database_url = env::var("DATABASE_URL").expect("DATABASE_URL required");
    let pool = PgPool::connect(&database_url).await.unwrap();

    // The CLI is an integration test — test the underlying DB operations
    let username = format!(
        "testuser_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let hash = "$argon2id$v=19$m=19456,t=2,p=1$test$test"; // dummy hash

    sqlx::query(
        "INSERT INTO users (id, username, password_hash, role) VALUES (gen_random_uuid(), $1, $2, 'operator')",
    )
    .bind(&username)
    .bind(hash)
    .execute(&pool)
    .await
    .expect("Insert user");

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE username = $1")
        .bind(&username)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1);

    // Cleanup
    sqlx::query("DELETE FROM users WHERE username = $1")
        .bind(&username)
        .execute(&pool)
        .await
        .unwrap();
}
