pub mod users;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "naughtywolf", about = "NaughtyWolf local lab platform")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// User management (local-only)
    User(users::UserCli),
    /// Start the web server (default)
    Serve,

    // ── hidden / future ──────────────────────
    /// Database migrations
    #[command(hide = true)]
    Migrate,
}
