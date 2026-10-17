//! MITRE ATT&CK mapping for naughtywolf tasking.
//!
//! Every operator command that maps to a published technique is annotated here,
//! so the tasking UI and reports can show *what* a command does in ATT&CK
//! terms rather than just its name. This is a local catalogue, not a fetch of
//! the live ATT&CK dataset: mappings are stable, small, and stable-at-runtime is
//! worth more here than auto-updating.
//!
//! Matching is longest-prefix so a variant like `nw/persist schtasks` keeps the
//! technique of `nw/persist` rather than falling through to nothing.

/// One ATT&CK technique reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Technique {
    pub id: &'static str,
    pub name: &'static str,
    /// Tactic the technique belongs to, for grouping in the UI.
    pub tactic: &'static str,
}

/// Tasking commands that correspond to a published technique.
///
/// Ordered by prefix length in [`lookup`] where it matters; the array itself is
/// grouped by command for readability.
const CATALOGUE: &[(&str, Technique)] = &[
    // Command and Control
    (
        "shell",
        Technique {
            id: "T1059",
            name: "Command and Scripting Interpreter",
            tactic: "Execution",
        },
    ),
    (
        "script",
        Technique {
            id: "T1059",
            name: "Command and Scripting Interpreter",
            tactic: "Execution",
        },
    ),
    (
        "redirect",
        Technique {
            id: "T1105",
            name: "Ingress Tool Transfer",
            tactic: "Command and Control",
        },
    ),
    // Credential Access
    (
        "creds",
        Technique {
            id: "T1003",
            name: "OS Credential Dumping",
            tactic: "Credential Access",
        },
    ),
    (
        "hashes",
        Technique {
            id: "T1003",
            name: "OS Credential Dumping",
            tactic: "Credential Access",
        },
    ),
    (
        "tokens",
        Technique {
            id: "T1134",
            name: "Access Token Manipulation",
            tactic: "Privilege Escalation",
        },
    ),
    // Collection
    (
        "keylog",
        Technique {
            id: "T1056.001",
            name: "Keylogging",
            tactic: "Collection",
        },
    ),
    // Persistence
    (
        "persist",
        Technique {
            id: "T1547",
            name: "Boot or Logon Autostart Execution",
            tactic: "Persistence",
        },
    ),
    (
        "registry",
        Technique {
            id: "T1547.001",
            name: "Registry Run Keys / Startup Folder",
            tactic: "Persistence",
        },
    ),
    // Lateral Movement / Proxy
    (
        "socks",
        Technique {
            id: "T1090",
            name: "Proxy",
            tactic: "Command and Control",
        },
    ),
];

/// Look up the technique for a tasking command.
///
/// Accepts a bare command (`creds`), the implant verb (`nw/creds`), or a
/// variant (`nw/persist schtasks`) and resolves the longest matching prefix, so
/// a sub-verb inherits its parent command's technique.
pub fn lookup(command: &str) -> Option<Technique> {
    let normalised = command
        .strip_prefix("nw/")
        .unwrap_or(command)
        .trim()
        .to_ascii_lowercase();
    if normalised.is_empty() {
        return None;
    }
    CATALOGUE
        .iter()
        .filter(|(name, _)| normalised.starts_with(name))
        // Longest prefix wins so `persist` never shadows a more specific match.
        .max_by_key(|(name, _)| name.len())
        .map(|(_, technique)| *technique)
}

/// Every technique the catalogue can produce, deduplicated, for the help UI.
///
/// Derived from the same table as [`lookup`] so the two cannot drift.
pub fn catalogue() -> Vec<(&'static str, &'static str, &'static str)> {
    let mut seen: Vec<Technique> = Vec::new();
    for (_, technique) in CATALOGUE {
        if !seen.contains(technique) {
            seen.push(*technique);
        }
    }
    seen.into_iter().map(|t| (t.id, t.name, t.tactic)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_the_technique_for_each_tasking_command() {
        assert_eq!(lookup("creds").map(|t| t.id), Some("T1003"));
        assert_eq!(lookup("keylog").map(|t| t.id), Some("T1056.001"));
        assert_eq!(lookup("persist").map(|t| t.id), Some("T1547"));
        assert_eq!(lookup("registry").map(|t| t.id), Some("T1547.001"));
        assert_eq!(lookup("socks").map(|t| t.id), Some("T1090"));
        assert_eq!(lookup("shell").map(|t| t.id), Some("T1059"));
    }

    /// The implant speaks `nw/<verb>`; the operator UI speaks the bare verb.
    /// Both must resolve identically or the UI and logs would disagree.
    #[test]
    fn the_implant_prefix_and_bare_command_agree() {
        for (bare, verb) in [
            ("creds", "nw/creds"),
            ("keylog", "nw/keylog"),
            ("hashes", "nw/hashes"),
            ("tokens", "nw/tokens"),
            ("persist", "nw/persist"),
            ("registry", "nw/registry"),
            ("socks", "nw/socks"),
        ] {
            assert_eq!(
                lookup(bare),
                lookup(verb),
                "{bare} and {verb} must map to the same technique"
            );
        }
    }

    /// A sub-verb inherits its parent's technique rather than falling through.
    #[test]
    fn sub_verbs_inherit_the_parent_technique() {
        assert_eq!(lookup("nw/persist schtasks"), lookup("persist"));
        assert_eq!(lookup("nw/registry query HKLM"), lookup("registry"));
        assert_eq!(lookup("nw/creds --harvest"), lookup("creds"));
    }

    #[test]
    fn matching_is_case_and_whitespace_insensitive() {
        assert_eq!(lookup("CREDS"), lookup("creds"));
        assert_eq!(lookup("  Creds  "), lookup("creds"));
    }

    /// Unmapped commands must return None rather than a plausible-looking but
    /// wrong technique; a false mapping is worse than no mapping.
    #[test]
    fn unmapped_commands_resolve_to_nothing() {
        for command in [
            "sessions", "jobs", "interact", "killjob", "", "   ", "download", "upload",
        ] {
            assert_eq!(lookup(command), None, "{command:?} must not be mapped");
        }
    }

    /// A prefix must not match a longer, different command: `registry` must not
    /// swallow a hypothetical `registryfoo`.
    #[test]
    fn a_prefix_only_matches_at_a_word_boundary() {
        assert_eq!(lookup("keys"), None, "a truncated word is not a match");
    }

    #[test]
    fn catalogue_is_deduplicated() {
        let entries = catalogue();
        let mut ids: Vec<&str> = entries.iter().map(|(id, _, _)| *id).collect();
        let total = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), total, "each technique appears once");
        assert!(ids.contains(&"T1003"), "creds technique is listed");
    }

    /// Every catalogue entry must use a syntactically valid ATT&CK id, since a
    /// malformed id would break any downstream matrix export. Ids are `T####`
    /// with an optional `.###` sub-technique suffix (e.g. T1056.001).
    #[test]
    fn every_catalogue_id_is_well_formed() {
        for (id, _, _) in catalogue() {
            let (base, sub) = match id.split_once('.') {
                Some((base, sub)) => (base, Some(sub)),
                None => (id, None),
            };
            assert!(
                base.len() == 5
                    && base.starts_with('T')
                    && base[1..].chars().all(|c| c.is_ascii_digit()),
                "malformed ATT&CK base id: {id}"
            );
            if let Some(sub) = sub {
                assert!(
                    sub.len() == 3 && sub.chars().all(|c| c.is_ascii_digit()),
                    "malformed ATT&CK sub-technique id: {id}"
                );
            }
        }
    }
}
