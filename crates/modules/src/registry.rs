use std::collections::HashMap;
use std::sync::Arc;

use crate::module::Role;
use crate::{Module, ModuleError};

/// Thread-safe, read-only-after-init module registry. Modules are registered
/// once at startup (by the server and the implant independently — they share
/// the trait, not the instances). Lookups are O(1) by `nw/<name>`.
#[derive(Default)]
pub struct Registry {
    modules: HashMap<String, Arc<dyn Module>>,
    order: Vec<String>,
}

/// Shared handle, same pattern as `SharedQueue`/`SharedRegistry` elsewhere.
pub type SharedRegistry = Arc<Registry>;

impl Registry {
    pub fn new() -> Self {
        Registry {
            modules: HashMap::new(),
            order: Vec::new(),
        }
    }

    /// Register a module implementation. Called during startup only.
    pub fn register(&mut self, module: impl Module + 'static) {
        let name = module.name().to_string();
        self.order.push(name.clone());
        self.modules.insert(name, Arc::new(module));
    }

    /// Look up a module by its `nw/<name>` command string.
    pub fn get(&self, name: &str) -> Result<Arc<dyn Module>, ModuleError> {
        self.modules
            .get(name)
            .cloned()
            .ok_or_else(|| ModuleError::NotFound(name.to_string()))
    }

    /// All registered module names, in registration order.
    pub fn names(&self) -> Vec<&str> {
        self.order.iter().map(|s| s.as_str()).collect()
    }

    /// All registered modules — for catalog/help listings.
    pub fn all(&self) -> Vec<Arc<dyn Module>> {
        self.order
            .iter()
            .filter_map(|n| self.modules.get(n).cloned())
            .collect()
    }

    /// Check whether an operator with `role` can run `name`.
    pub fn is_authorized(&self, name: &str, role: Role) -> bool {
        match self.get(name) {
            Ok(m) => role.allows(m.required_role()),
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    struct FakeModule {
        name: String,
        role: Role,
    }

    impl Module for FakeModule {
        fn name(&self) -> &str {
            &self.name
        }
        fn description(&self) -> &str {
            "a fake module for testing"
        }
        fn args(&self) -> &[&str] {
            &[]
        }
        fn required_role(&self) -> Role {
            self.role
        }
        fn run(&self, _args: &[String]) -> crate::ModuleResult {
            crate::ModuleResult::ok(&self.name, Uuid::new_v4(), serde_json::Value::Null)
        }
    }

    #[test]
    fn register_and_lookup() {
        let mut reg = Registry::new();
        reg.register(FakeModule {
            name: "nw/fake".into(),
            role: Role::Viewer,
        });
        assert!(reg.get("nw/fake").is_ok());
        assert!(reg.get("nw/missing").is_err());
        assert_eq!(reg.names(), vec!["nw/fake"]);
    }

    #[test]
    fn authorization_follows_role_hierarchy() {
        let mut reg = Registry::new();
        reg.register(FakeModule {
            name: "nw/secret".into(),
            role: Role::Admin,
        });
        reg.register(FakeModule {
            name: "nw/public".into(),
            role: Role::Viewer,
        });

        assert!(reg.is_authorized("nw/secret", Role::Admin));
        assert!(!reg.is_authorized("nw/secret", Role::Operator));
        assert!(!reg.is_authorized("nw/secret", Role::Viewer));
        assert!(reg.is_authorized("nw/public", Role::Admin));
        assert!(reg.is_authorized("nw/public", Role::Operator));
        assert!(reg.is_authorized("nw/public", Role::Viewer));
    }
}
