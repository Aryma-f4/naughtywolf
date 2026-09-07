//! Automated task sequences / operator playbooks.
//!
//! A [`Script`] is a directed acyclic graph of [`Step`]s. Each step dispatches a
//! task (e.g. `nw/download`, `shell <cmd>`) to a target session and optionally
//! gates on the result. Steps can depend on other steps (by id), retry on
//! failure, and skip when a condition is not met.
//!
//! Scripts are authored in a simple text format (see [`Script::parse`]) and
//! queued via the `nw/script <file>` console command.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

/// A single task in an automation script.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Step {
    /// Unique name within the script (used by `depends_on`).
    pub name: String,
    /// The command to dispatch to the implant (e.g. `nw/download`, `shell`).
    pub command: String,
    /// Arguments for the command.
    pub args: Vec<String>,
    /// Task timeout in milliseconds (0 = no timeout).
    pub timeout_ms: u64,
    /// Maximum number of attempts before the step is marked failed.
    pub retry: u32,
    /// Optional condition: a substring that must appear in a prior step's
    /// stdout for this step to run. Empty = always run.
    pub require: Option<String>,
    /// Steps whose completion (success) is required before this one runs.
    pub depends_on: Vec<String>,
}

/// A parsed automation script: ordered steps with dependency graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Script {
    pub name: String,
    pub steps: Vec<Step>,
}

/// Outcome of executing a single step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResult {
    pub step: String,
    pub task_id: uuid::Uuid,
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
    pub attempts: u32,
}

/// Error returned when parsing or validating a script.
#[derive(Debug, thiserror::Error)]
pub enum ScriptError {
    #[error("parse error on line {line}: {msg}")]
    Parse { line: usize, msg: String },
    #[error("step '{0}' depends on unknown step")]
    UnknownDependency(String),
    #[error("cycle detected in dependency graph")]
    Cycle,
    #[error("duplicate step name: {0}")]
    Duplicate(String),
}

impl Step {
    fn default_timeout() -> u64 {
        30_000
    }
}

impl Default for Step {
    fn default() -> Self {
        Step {
            name: String::new(),
            command: String::new(),
            args: Vec::new(),
            timeout_ms: Step::default_timeout(),
            retry: 0,
            require: None,
            depends_on: Vec::new(),
        }
    }
}

impl Script {
    /// Parse a text-format automation script.
    ///
    /// Format:
    ///   script: enum-lateral
    ///   step enum_hosts {
    ///       cmd: shell
    ///       args: enum_hosts.sh
    ///       retry: 2
    ///   }
    ///   step lateral {
    ///       cmd: nw/download
    ///       args: /etc/shadow
    ///       depends_on: [enum_hosts]
    ///       require: root
    ///   }
    pub fn parse(text: &str) -> Result<Self, ScriptError> {
        let mut name = String::new();
        let mut steps: Vec<Step> = Vec::new();
        let mut step_names: HashSet<String> = HashSet::new();
        let mut current: Option<Step> = None;

        for (lineno, raw_line) in text.lines().enumerate() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            // Top-level "script: <name>" directive.
            if line.starts_with("script:") {
                if current.is_some() {
                    // Flush any in-progress step.
                    let s = current.take().unwrap();
                    steps.push(s);
                }
                name = line["script:".len()..].trim().to_owned();
                continue;
            }

            // Begin a new step: "step <name> {"
            if let Some(rest) = line.strip_prefix("step ") {
                if current.is_some() {
                    let s = current.take().unwrap();
                    steps.push(s);
                }
                let name_str = rest.trim_end_matches('{').trim();
                if name_str.is_empty() {
                    return Err(ScriptError::Parse {
                        line: lineno + 1,
                        msg: "step name missing".into(),
                    });
                }
                if !step_names.insert(name_str.to_owned()) {
                    return Err(ScriptError::Duplicate(name_str.to_owned()));
                }
                current = Some(Step {
                    name: name_str.to_owned(),
                    ..Default::default()
                });
                continue;
            }

            // Closing brace ends the current step.
            if line == "}" {
                if let Some(s) = current.take() {
                    steps.push(s);
                } else {
                    return Err(ScriptError::Parse {
                        line: lineno + 1,
                        msg: "unexpected }".into(),
                    });
                }
                continue;
            }

            // Key-value inside a step.
            if current.is_none() {
                return Err(ScriptError::Parse {
                    line: lineno + 1,
                    msg: "directive outside of step".into(),
                });
            }

            let step = current.as_mut().unwrap();
            if let Some((key, val)) = line.split_once(':') {
                let key = key.trim();
                let val = val.trim();
                match key {
                    "cmd" => step.command = val.to_owned(),
                    "args" => {
                        step.args = val.split_whitespace().map(|s| s.to_owned()).collect();
                    }
                    "timeout_ms" => {
                        step.timeout_ms = val.parse::<u64>().map_err(|_| ScriptError::Parse {
                            line: lineno + 1,
                            msg: format!("invalid timeout_ms: {val}"),
                        })?;
                    }
                    "retry" => {
                        step.retry = val.parse::<u32>().map_err(|_| ScriptError::Parse {
                            line: lineno + 1,
                            msg: format!("invalid retry: {val}"),
                        })?;
                    }
                    "require" => step.require = Some(val.to_owned()),
                    "depends_on" => {
                        let cleaned = val.trim_matches(|c| c == '[' || c == ']');
                        step.depends_on = cleaned
                            .split(',')
                            .map(|s| s.trim().to_owned())
                            .filter(|s| !s.is_empty())
                            .collect();
                    }
                    _ => {
                        return Err(ScriptError::Parse {
                            line: lineno + 1,
                            msg: format!("unknown key: {key}"),
                        });
                    }
                }
            }
        }

        // Flush trailing step.
        if let Some(s) = current.take() {
            steps.push(s);
        }

        if name.is_empty() {
            name = "unnamed".to_owned();
        }

        let script = Script { name, steps };
        script.validate()?;
        Ok(script)
    }

    /// Validate the dependency graph: no unknown deps, no cycles, no dups.
    fn validate(&self) -> Result<(), ScriptError> {
        let names: HashMap<&str, &Step> = self.steps.iter().map(|s| (s.name.as_str(), s)).collect();

        for step in &self.steps {
            for dep in &step.depends_on {
                if !names.contains_key(dep.as_str()) {
                    return Err(ScriptError::UnknownDependency(dep.clone()));
                }
            }
        }

        // Cycle detection via topological sort (Kahn's algorithm).
        let mut indegree: HashMap<String, usize> = self
            .steps
            .iter()
            .map(|s| (s.name.clone(), s.depends_on.len()))
            .collect();
        let mut queue: Vec<String> = indegree
            .iter()
            .filter(|(_, deg)| **deg == 0)
            .map(|(n, _)| n.clone())
            .collect();
        let mut visited = 0;
        while let Some(node) = queue.pop() {
            visited += 1;
            for step in &self.steps {
                if step.depends_on.iter().any(|d| d == &node) {
                    let deg = indegree.get_mut(&step.name).unwrap();
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push(step.name.clone());
                    }
                }
            }
        }

        if visited < self.steps.len() {
            return Err(ScriptError::Cycle);
        }

        Ok(())
    }

    /// Return steps in topological order (dependencies before dependents).
    pub fn topo_sorted(&self) -> Vec<&Step> {
        let mut indegree: HashMap<String, usize> = self
            .steps
            .iter()
            .map(|s| (s.name.clone(), s.depends_on.len()))
            .collect();
        let mut available: Vec<&Step> = self
            .steps
            .iter()
            .filter(|s| indegree.get(s.name.as_str()) == Some(&0))
            .collect();
        let mut result = Vec::new();

        while let Some(step) = available.pop() {
            result.push(step);
            for other in &self.steps {
                if other.depends_on.iter().any(|d| d == &step.name) {
                    let deg = indegree.get_mut(&other.name).unwrap();
                    *deg -= 1;
                    if *deg == 0 {
                        available.push(other);
                    }
                }
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_script() {
        let text = r#"
            script: test
            step alpha {
                cmd: shell
                args: whoami
            }
            step beta {
                cmd: nw/hashes
                args: /etc/passwd
                depends_on: [alpha]
            }
        "#;
        let script = Script::parse(text).unwrap();
        assert_eq!(script.name, "test");
        assert_eq!(script.steps.len(), 2);
        assert_eq!(script.steps[0].command, "shell");
        assert_eq!(script.steps[1].command, "nw/hashes");
        assert_eq!(script.steps[1].depends_on, vec!["alpha"]);
    }

    #[test]
    fn cycle_is_rejected() {
        let text = r#"
            script: cyc2
            step a {
                cmd: shell
                args: a
                depends_on: [b]
            }
            step b {
                cmd: shell
                args: b
                depends_on: [a]
            }
        "#;
        assert!(matches!(Script::parse(text), Err(ScriptError::Cycle)));
    }

    #[test]
    fn unknown_dependency_rejected() {
        let text = r#"
            script: bad
            step a {
                cmd: shell
                args: a
                depends_on: [ghost]
            }
        "#;
        assert!(matches!(
            Script::parse(text),
            Err(ScriptError::UnknownDependency(_))
        ));
    }

    #[test]
    fn topo_order_respects_dependencies() {
        let text = r#"
            script: topo
            step c {
                cmd: shell
                args: c
                depends_on: [a, b]
            }
            step a {
                cmd: shell
                args: a
            }
            step b {
                cmd: shell
                args: b
            }
        "#;
        let script = Script::parse(text).unwrap();
        let order: Vec<&str> = script
            .topo_sorted()
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        let a_pos = order.iter().position(|&n| n == "a").unwrap();
        let b_pos = order.iter().position(|&n| n == "b").unwrap();
        let c_pos = order.iter().position(|&n| n == "c").unwrap();
        assert!(a_pos < c_pos);
        assert!(b_pos < c_pos);
    }
}
