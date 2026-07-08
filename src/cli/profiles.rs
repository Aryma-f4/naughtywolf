use clap::{Parser, Subcommand};
use sqlx::PgPool;
use uuid::Uuid;

use crate::sliver::profiles;

#[derive(Parser)]
pub struct ProfileCli {
    #[command(subcommand)]
    pub action: ProfileAction,
}

#[derive(Subcommand)]
pub enum ProfileAction {
    /// Scan for Sliver operator configs in config dir
    Scan {
        #[arg(long)]
        config_dir: Option<String>,
    },
    /// Assign a user to a profile
    Assign {
        #[arg(long)]
        username: String,
        #[arg(long)]
        profile: String,
    },
    /// List profiles
    List,
}

impl ProfileCli {
    pub async fn execute(
        self,
        pool: &PgPool,
        config_dir: &std::path::Path,
    ) -> anyhow::Result<()> {
        match self.action {
            ProfileAction::Scan { config_dir: dir } => {
                let dir = dir
                    .map(std::path::PathBuf::from)
                    .unwrap_or(config_dir.to_path_buf());
                let found = profiles::scan_config_dir(&dir)?;
                if found.is_empty() {
                    println!("No Sliver configs found in {}", dir.display());
                    return Ok(());
                }
                for p in &found {
                    // Upsert profile
                    let exists = sqlx::query_scalar::<_, i64>(
                        "SELECT COUNT(*) FROM sliver_profiles WHERE name = $1",
                    )
                    .bind(&p.name)
                    .fetch_one(pool)
                    .await?;

                    if exists == 0 {
                        sqlx::query(
                            "INSERT INTO sliver_profiles (id, name, config_path, operator_name, lhost, lport) \
                             VALUES ($1, $2, $3, $4, $5, $6)",
                        )
                        .bind(Uuid::new_v4())
                        .bind(&p.name)
                        .bind(p.config_path.to_string_lossy().to_string())
                        .bind(&p.operator_name)
                        .bind(&p.lhost)
                        .bind(p.lport as i32)
                        .execute(pool)
                        .await?;
                        println!(
                            "Added profile: {} ({}@{}:{})",
                            p.name, p.operator_name, p.lhost, p.lport
                        );
                    } else {
                        // Update existing
                        sqlx::query(
                            "UPDATE sliver_profiles SET config_path=$1, operator_name=$2, lhost=$3, lport=$4 \
                             WHERE name=$5",
                        )
                        .bind(p.config_path.to_string_lossy().to_string())
                        .bind(&p.operator_name)
                        .bind(&p.lhost)
                        .bind(p.lport as i32)
                        .bind(&p.name)
                        .execute(pool)
                        .await?;
                        println!("Updated profile: {}", p.name);
                    }
                }
                if found.is_empty() {
                    println!("No Sliver operator configs found.");
                } else {
                    println!("Scanned {} config(s)", found.len());
                }
                Ok(())
            }
            ProfileAction::Assign { username, profile } => {
                let user = sqlx::query_as::<_, crate::db::models::User>(
                    "SELECT * FROM users WHERE username = $1",
                )
                .bind(&username)
                .fetch_optional(pool)
                .await?
                .ok_or_else(|| anyhow::anyhow!("User '{username}' not found"))?;

                let profile_row = sqlx::query_as::<_, crate::db::models::SliverProfile>(
                    "SELECT * FROM sliver_profiles WHERE name = $1",
                )
                .bind(&profile)
                .fetch_optional(pool)
                .await?
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Profile '{profile}' not found. Run 'profile scan' first."
                    )
                })?;

                // Check if assignment exists
                let exists = sqlx::query_scalar::<_, i64>(
                    "SELECT COUNT(*) FROM user_sliver_profiles WHERE user_id = $1 AND profile_id = $2",
                )
                .bind(user.id)
                .bind(profile_row.id)
                .fetch_one(pool)
                .await?;

                if exists == 0 {
                    sqlx::query(
                        "INSERT INTO user_sliver_profiles (user_id, profile_id, default_profile) VALUES ($1, $2, true)",
                    )
                    .bind(user.id)
                    .bind(profile_row.id)
                    .execute(pool)
                    .await?;
                    println!("Assigned user '{username}' to profile '{profile}'");
                } else {
                    println!("User '{username}' already assigned to profile '{profile}'");
                }
                Ok(())
            }
            ProfileAction::List => {
                let profiles = sqlx::query_as::<_, crate::db::models::SliverProfile>(
                    "SELECT sp.* FROM sliver_profiles sp ORDER BY sp.name",
                )
                .fetch_all(pool)
                .await?;

                if profiles.is_empty() {
                    println!("No profiles. Run 'profile scan' first.");
                    return Ok(());
                }

                println!(
                    "{:<5} {:<20} {:<15} {:<15} Port",
                    "ID", "Name", "Operator", "Host",
                );
                println!("{}", "-".repeat(70));
                for p in &profiles {
                    println!(
                        "{:<5} {:<20} {:<15} {:<15} {}",
                        &p.id.to_string()[..8],
                        p.name,
                        p.operator_name,
                        p.lhost,
                        p.lport,
                    );
                }
                Ok(())
            }
        }
    }
}
