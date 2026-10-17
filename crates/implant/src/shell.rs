//! Interactive shells the operator attaches over the live channel.
//!
//! A shell here is a real child process with a real terminal when the platform
//! offers one (`forkpty` on Unix, ConPTY on Windows), so line editing, job
//! control, colour and full-screen programs behave the way they do in a normal
//! terminal. Where a PTY cannot be created the same child is run with pipes
//! instead: still interactive, just without the terminal attributes.
//!
//! Output is pumped by dedicated threads because a PTY master is a blocking
//! file descriptor; each chunk is handed to the sink as raw bytes and the
//! live-channel layer takes care of framing and sealing them.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use uuid::Uuid;

/// One live shell: its identity, how to write to it, and how to stop it.
pub struct InteractiveShell {
    pub program: String,
    /// False when the child fell back to pipes.
    pub pty: bool,
    /// Writes are serialized so two operators (or a prompt and a paste) cannot
    /// interleave halves of the same line.
    writer: std::sync::Mutex<Box<dyn Write + Send>>,
    killer: Box<dyn Fn() + Send + Sync>,
}

impl InteractiveShell {
    /// Write operator input, including control characters (`\r`, `\x03`, …).
    pub fn write(&self, data: &[u8]) -> Result<(), String> {
        let mut writer = self.writer.lock().expect("shell writer poisoned");
        writer
            .write_all(data)
            .and_then(|()| writer.flush())
            .map_err(|error| format!("write shell input: {error}"))
    }

    pub fn kill(&self) {
        (self.killer)();
    }
}

/// Bytes (or an exit) produced by a running shell.
pub enum ShellOutput {
    /// `stderr` is only ever true on the pipe fallback; a terminal merges the
    /// two streams into one, which is what an interactive session expects.
    Data(Vec<u8>, bool),
    Closed(Option<i32>),
}

/// The shells currently attached to this session.
#[derive(Default)]
pub struct ShellRegistry {
    shells: Mutex<HashMap<Uuid, InteractiveShell>>,
}

impl ShellRegistry {
    /// Start a shell, reporting whether it got a real terminal.
    pub fn open(
        &self,
        id: Uuid,
        program: &str,
        args: &[String],
        sink: Arc<dyn Fn(ShellOutput) + Send + Sync>,
    ) -> Result<bool, String> {
        // Re-opening an id replaces the previous shell, so a retried attach
        // cannot leave an orphaned child behind.
        self.close(id);
        let shell = match spawn_pty(program, args, sink.clone()) {
            Ok(shell) => shell,
            Err(pty_error) => {
                tracing::debug!(%pty_error, "pty unavailable; falling back to pipes");
                spawn_pipe(program, args, sink)?
            }
        };
        let pty = shell.pty;
        self.shells
            .lock()
            .expect("shell registry poisoned")
            .insert(id, shell);
        Ok(pty)
    }

    pub fn write(&self, id: Uuid, data: &[u8]) -> Result<(), String> {
        let shells = self.shells.lock().expect("shell registry poisoned");
        let shell = shells
            .get(&id)
            .ok_or_else(|| format!("no shell {id} is attached"))?;
        shell.write(data)
    }

    /// Stop one shell. Reports whether one was attached.
    pub fn close(&self, id: Uuid) -> bool {
        let shell = self
            .shells
            .lock()
            .expect("shell registry poisoned")
            .remove(&id);
        match shell {
            Some(shell) => {
                shell.kill();
                true
            }
            None => false,
        }
    }

    pub fn close_all(&self) {
        let mut shells = self.shells.lock().expect("shell registry poisoned");
        for (_, shell) in shells.drain() {
            shell.kill();
        }
    }

    pub fn is_open(&self, id: Uuid) -> bool {
        self.shells
            .lock()
            .expect("shell registry poisoned")
            .contains_key(&id)
    }

    pub fn is_empty(&self) -> bool {
        self.shells
            .lock()
            .expect("shell registry poisoned")
            .is_empty()
    }
}

/// The command an operator gets when they attach without naming one.
pub fn default_program(os: &str) -> (&'static str, Vec<String>) {
    if os.to_ascii_lowercase().contains("windows") {
        ("cmd.exe", vec!["/Q".to_owned()])
    } else {
        ("/bin/sh", vec!["-i".to_owned()])
    }
}

fn spawn_pty(
    program: &str,
    args: &[String],
    sink: Arc<dyn Fn(ShellOutput) + Send + Sync>,
) -> Result<InteractiveShell, String> {
    use portable_pty::{CommandBuilder, PtySize};

    let pair = portable_pty::native_pty_system()
        .openpty(PtySize {
            rows: 40,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| format!("openpty: {error}"))?;

    let mut command = CommandBuilder::new(program);
    command.args(args);
    // Terminal semantics: give the child a colour-capable TERM so interactive
    // shells and `ls --color` behave the way they do in a real terminal.
    command.env("TERM", "xterm-256color");

    let child = pair
        .slave
        .spawn_command(command)
        .map_err(|error| format!("spawn pty child: {error}"))?;
    // The child holds its own handle to the slave side; keeping a second one
    // here would stop the master from ever reporting EOF.
    drop(pair.slave);

    let reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| format!("pty reader: {error}"))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|error| format!("pty writer: {error}"))?;

    let killer = Mutex::new(child.clone_killer());
    let exit_sink = sink.clone();
    std::thread::spawn(move || {
        // A terminal merges stderr into stdout, so both arrive unmarked.
        pump(reader, sink, false);
        // A pty gives no exit status; the master closing means the child is
        // gone, which is all the operator needs to see.
        exit_sink(ShellOutput::Closed(None));
    });
    // Release this handle once the pump owns the reader, so nothing else keeps
    // the master alive after the child exits.
    drop(child);

    Ok(InteractiveShell {
        program: program.to_owned(),
        pty: true,
        writer: std::sync::Mutex::new(Box::new(writer)),
        killer: Box::new(move || {
            if let Ok(mut killer) = killer.lock() {
                let _ = killer.kill();
            }
        }),
    })
}

fn spawn_pipe(
    program: &str,
    args: &[String],
    sink: Arc<dyn Fn(ShellOutput) + Send + Sync>,
) -> Result<InteractiveShell, String> {
    use std::process::{Command, Stdio};

    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("spawn shell: {error}"))?;

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| "shell stdin unavailable".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "shell stdout unavailable".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "shell stderr unavailable".to_string())?;

    let mut readers: Vec<(Box<dyn Read + Send>, bool)> =
        vec![(Box::new(stdout), false), (Box::new(stderr), true)];
    for (stream, is_err) in readers.drain(..) {
        let sink = sink.clone();
        std::thread::spawn(move || pump(stream, sink, is_err));
    }

    // The child is shared: the killer stops it on demand while a waiter thread
    // watches for the exit so the operator sees the shell close exactly once.
    let child = Arc::new(Mutex::new(child));
    let watched = Arc::clone(&child);
    let exit_sink = sink;
    std::thread::spawn(move || {
        loop {
            let finished = match watched.lock() {
                Ok(mut child) => child.try_wait().ok().flatten(),
                Err(_) => return,
            };
            if let Some(status) = finished {
                exit_sink(ShellOutput::Closed(status.code()));
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    });

    Ok(InteractiveShell {
        program: program.to_owned(),
        pty: false,
        writer: std::sync::Mutex::new(Box::new(stdin)),
        killer: Box::new(move || {
            if let Ok(mut child) = child.lock() {
                let _ = child.kill();
            }
        }),
    })
}

/// Read a stream to EOF, handing every chunk to the sink.
fn pump<R: Read + Send + 'static>(
    mut stream: R,
    sink: Arc<dyn Fn(ShellOutput) + Send + Sync>,
    is_err: bool,
) {
    let mut buffer = [0u8; 8192];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => sink(ShellOutput::Data(buffer[..read].to_vec(), is_err)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[cfg(unix)]
    fn echo_command() -> (&'static str, Vec<String>) {
        ("/bin/sh", vec!["-i".into()])
    }
    #[cfg(windows)]
    fn echo_command() -> (&'static str, Vec<String>) {
        ("cmd.exe".into(), vec!["/Q".into()])
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn attached_shell_echoes_input_and_reports_exit() {
        let (program, args) = echo_command();
        let collected = Arc::new(StdMutex::new(Vec::new()));
        let closed = Arc::new(AtomicBool::new(false));
        let sink_collected = Arc::clone(&collected);
        let sink_closed = Arc::clone(&closed);
        let registry = ShellRegistry::default();
        let id = Uuid::new_v4();
        let sink: Arc<dyn Fn(ShellOutput) + Send + Sync> = Arc::new(move |output| match output {
            ShellOutput::Data(bytes, _) => sink_collected.lock().unwrap().extend_from_slice(&bytes),
            ShellOutput::Closed(_) => {
                sink_closed.store(true, Ordering::SeqCst);
            }
        });

        registry
            .open(id, program, &args, sink)
            .expect("shell opens");

        // Wait for the prompt before typing: a shell that is still starting
        // treats early input as part of its own initialization.
        let started = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        while collected.lock().unwrap().is_empty() && tokio::time::Instant::now() < started {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }

        // Output is asynchronous: wait for the command to answer rather than
        // sleeping a guessed interval, and ask again if the shell swallowed it.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            registry
                .write(id, b"echo naughtYWolfProbe\n")
                .expect("input is written");
            let answered = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                let seen = String::from_utf8_lossy(&collected.lock().unwrap()).to_lowercase();
                if seen.contains("naughtywolfprobe") {
                    break;
                }
                if tokio::time::Instant::now() >= answered
                    || tokio::time::Instant::now() >= deadline
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            let seen = String::from_utf8_lossy(&collected.lock().unwrap()).to_lowercase();
            if seen.contains("naughtywolfprobe") {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "probe output never arrived, saw {seen:?}"
            );
        }

        assert!(registry.close(id), "the attached shell is stopped");
        assert!(!registry.is_open(id));
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
        while !closed.load(Ordering::SeqCst) && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(
            closed.load(Ordering::SeqCst),
            "shell never reported an exit"
        );
    }

    #[tokio::test]
    async fn closing_an_unknown_shell_reports_nothing_was_attached() {
        let registry = ShellRegistry::default();
        assert!(!registry.close(Uuid::new_v4()));
        assert!(registry.is_empty());
    }
}
