use clap::{Parser, Subcommand};
use sqlx::{Row, SqlitePool};
use std::io::{self, IsTerminal};
use uuid::Uuid;

use crate::auth::password;
use crate::auth::rbac::Role;

#[derive(Parser)]
pub struct UserCli {
    #[command(subcommand)]
    pub action: UserAction,
}

#[derive(Subcommand)]
pub enum UserAction {
    /// Create a new user
    Create {
        /// Username
        #[arg(long)]
        username: String,
        /// Role: admin, operator, viewer
        #[arg(long, default_value = "operator")]
        role: String,
        /// Read the password from standard input (for CI only)
        #[arg(long)]
        password_stdin: bool,
    },
    /// List all users
    List,
    /// Reset a user's password
    ResetPassword {
        #[arg(long)]
        username: String,
        #[arg(long)]
        password_stdin: bool,
    },
    /// Disable a user account
    Disable {
        #[arg(long)]
        username: String,
    },
}

impl UserCli {
    pub async fn execute(self, pool: &SqlitePool) -> anyhow::Result<()> {
        match self.action {
            UserAction::Create {
                username,
                role,
                password_stdin,
            } => Self::create_user(pool, &username, &role, password_stdin).await,
            UserAction::List => Self::list_users(pool).await,
            UserAction::ResetPassword {
                username,
                password_stdin,
            } => Self::reset_password(pool, &username, password_stdin).await,
            UserAction::Disable { username } => Self::disable_user(pool, &username).await,
        }
    }

    async fn create_user(
        pool: &SqlitePool,
        username: &str,
        role: &str,
        password_stdin: bool,
    ) -> anyhow::Result<()> {
        let role = role
            .parse::<Role>()
            .map_err(|_| anyhow::anyhow!("Invalid role: {role}. Use admin, operator, or viewer"))?;

        let password = read_password(password_stdin, "Password: ")?;

        let hash = password::hash_password(&password)
            .map_err(|e| anyhow::anyhow!("Failed to hash password: {e}"))?;
        let id = Uuid::new_v4().to_string();
        let mut transaction = pool.begin().await?;

        sqlx::query("INSERT INTO users (id, username, password_hash, role) VALUES (?, ?, ?, ?)")
            .bind(&id)
            .bind(username)
            .bind(&hash)
            .bind(role.to_string())
            .execute(&mut *transaction)
            .await?;

        sqlx::query(
            "INSERT INTO audit_events \
             (id, actor_id, operation_id, action, target_type, target_id, parameter_summary, outcome, correlation_id) \
             VALUES (?, NULL, NULL, 'user.created', 'user', ?, ?, 'success', ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(&id)
        .bind(format!("username={username}, role={role}"))
        .bind(Uuid::new_v4().to_string())
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;

        println!("User '{username}' created with role '{role}' (id: {id})");
        Ok(())
    }

    async fn list_users(pool: &SqlitePool) -> anyhow::Result<()> {
        let users = sqlx::query(
            "SELECT id, username, role, disabled, created_at FROM users ORDER BY created_at",
        )
        .fetch_all(pool)
        .await?;

        if users.is_empty() {
            println!("No users found.");
            return Ok(());
        }

        println!(
            "{:<5} {:<20} {:<10} {:<8} Created",
            "ID", "Username", "Role", "Disabled",
        );
        println!("{}", "-".repeat(70));
        for user in &users {
            let id: String = user.try_get("id")?;
            let username: String = user.try_get("username")?;
            let role: String = user.try_get("role")?;
            let disabled: bool = user.try_get("disabled")?;
            let created_at: String = user.try_get("created_at")?;
            println!(
                "{:<5} {:<20} {:<10} {:<8} {}",
                &id[..id.len().min(8)],
                username,
                role,
                if disabled { "yes" } else { "no" },
                created_at,
            );
        }
        Ok(())
    }

    async fn reset_password(
        pool: &SqlitePool,
        username: &str,
        password_stdin: bool,
    ) -> anyhow::Result<()> {
        let password = read_password(password_stdin, "New password: ")?;

        let hash = password::hash_password(&password)
            .map_err(|e| anyhow::anyhow!("Failed to hash password: {e}"))?;
        let updated = sqlx::query(
            "UPDATE users SET password_hash = ?, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE username = ?",
        )
        .bind(&hash)
        .bind(username)
        .execute(pool)
        .await?
        .rows_affected();

        if updated == 0 {
            anyhow::bail!("User '{username}' not found");
        }
        println!("Password reset for '{username}'");
        Ok(())
    }

    async fn disable_user(pool: &SqlitePool, username: &str) -> anyhow::Result<()> {
        let updated =
            sqlx::query("UPDATE users SET disabled = 1, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now') WHERE username = ?")
                .bind(username)
                .execute(pool)
                .await?
                .rows_affected();

        if updated == 0 {
            anyhow::bail!("User '{username}' not found");
        }
        println!("User '{username}' disabled");
        Ok(())
    }
}

fn read_password(password_stdin: bool, prompt: &str) -> anyhow::Result<String> {
    if password_stdin {
        let mut password = String::new();
        io::stdin().read_line(&mut password)?;
        return Ok(password.trim_end_matches(['\r', '\n']).to_owned());
    }

    if !io::stdin().is_terminal() {
        anyhow::bail!("password input requires a TTY or --password-stdin for CI");
    }

    Ok(rpassword::prompt_password(prompt)?)
}
