use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Role {
    Admin,
    Operator,
    Viewer,
}

impl Role {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "admin" => Some(Role::Admin),
            "operator" => Some(Role::Operator),
            "viewer" => Some(Role::Viewer),
            _ => None,
        }
    }

    pub fn can_perform(&self, required: &Role) -> bool {
        match (self, required) {
            (Role::Admin, _) => true,
            (Role::Operator, Role::Operator | Role::Viewer) => true,
            (Role::Operator, Role::Admin) => false,
            (Role::Viewer, Role::Viewer) => true,
            (Role::Viewer, _) => false,
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Role::Admin => write!(f, "admin"),
            Role::Operator => write!(f, "operator"),
            Role::Viewer => write!(f, "viewer"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_admin_can_do_everything() {
        assert!(Role::Admin.can_perform(&Role::Admin));
        assert!(Role::Admin.can_perform(&Role::Operator));
        assert!(Role::Admin.can_perform(&Role::Viewer));
    }

    #[test]
    fn test_operator_cannot_admin() {
        assert!(!Role::Operator.can_perform(&Role::Admin));
    }

    #[test]
    fn test_viewer_read_only() {
        assert!(!Role::Viewer.can_perform(&Role::Admin));
        assert!(!Role::Viewer.can_perform(&Role::Operator));
    }

    #[test]
    fn test_role_from_str() {
        assert_eq!(Role::from_str("admin"), Some(Role::Admin));
        assert_eq!(Role::from_str("VIEWER"), Some(Role::Viewer));
        assert_eq!(Role::from_str("unknown"), None);
    }
}
