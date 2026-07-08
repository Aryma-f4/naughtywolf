use clap::{Parser, Subcommand};
use sqlx::PgPool;
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
        /// Password (prompts if not provided)
        #[arg(long)]
        password: Option<String>,
    },
    /// List all users
    List,
    /// Reset a user's password
    ResetPassword {
        #[arg(long)]
        username: String,
        #[arg(long)]
        password: Option<String>,
    },
    /// Disable a user account
    Disable {
        #[arg(long)]
        username: String,
    },
}

impl UserCli {
    pub async fn execute(self, pool: &PgPool) -> anyhow::Result<()> {
        match self.action {
            UserAction::Create {
                username,
                role,
                password,
            } => Self::create_user(pool, &username, &role, password.as_deref()).await,
            UserAction::List => Self::list_users(pool).await,
            UserAction::ResetPassword { username, password } => {
                Self::reset_password(pool, &username, password.as_deref()).await
            }
            UserAction::Disable { username } => Self::disable_user(pool, &username).await,
        }
    }

    async fn create_user(
        pool: &PgPool,
        username: &str,
        role: &str,
        password: Option<&str>,
    ) -> anyhow::Result<()> {
        let role = role
            .parse::<Role>()
            .map_err(|_| anyhow::anyhow!("Invalid role: {role}. Use admin, operator, or viewer"))?;

        let password = match password {
            Some(p) => p.to_string(),
            None => rpassword::prompt_password("Password: ")?,
        };

        let hash = password::hash_password(&password)
            .map_err(|e| anyhow::anyhow!("Failed to hash password: {e}"))?;
        let id = Uuid::new_v4();

        sqlx::query(
            "INSERT INTO users (id, username, password_hash, role) VALUES ($1, $2, $3, $4)",
        )
        .bind(id)
        .bind(username)
        .bind(&hash)
        .bind(role.to_string())
        .execute(pool)
        .await?;

        println!("User '{username}' created with role '{role}' (id: {id})");
        Ok(())
    }

    async fn list_users(pool: &PgPool) -> anyhow::Result<()> {
        let users =
            sqlx::query_as::<_, crate::db::models::User>("SELECT * FROM users ORDER BY created_at")
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
            println!(
                "{:<5} {:<20} {:<10} {:<8} {}",
                &user.id.to_string()[..8],
                user.username,
                user.role,
                if user.disabled { "yes" } else { "no" },
                user.created_at.format("%Y-%m-%d %H:%M"),
            );
        }
        Ok(())
    }

    async fn reset_password(
        pool: &PgPool,
        username: &str,
        password: Option<&str>,
    ) -> anyhow::Result<()> {
        let password = match password {
            Some(p) => p.to_string(),
            None => rpassword::prompt_password("New password: ")?,
        };

        let hash = password::hash_password(&password)
            .map_err(|e| anyhow::anyhow!("Failed to hash password: {e}"))?;
        let updated = sqlx::query(
            "UPDATE users SET password_hash = $1, updated_at = now() WHERE username = $2",
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

    async fn disable_user(pool: &PgPool, username: &str) -> anyhow::Result<()> {
        let updated =
            sqlx::query("UPDATE users SET disabled = true, updated_at = now() WHERE username = $1")
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
