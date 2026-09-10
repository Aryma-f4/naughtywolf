use serde::{Deserialize, Serialize};

/// Workspace features understood by a callback. Older registrations omit this
/// object and therefore deserialize to a conservative, all-disabled value.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallbackCapabilities {
    #[serde(default)]
    pub process_browser: bool,
    #[serde(default)]
    pub file_browser: bool,
    #[serde(default)]
    pub file_transfer: bool,
    #[serde(default)]
    pub task_ack: bool,
}

#[cfg(test)]
mod tests {
    use super::CallbackCapabilities;

    #[test]
    fn legacy_capabilities_default_to_disabled() {
        let capabilities = CallbackCapabilities::default();

        assert!(!capabilities.process_browser);
        assert!(!capabilities.file_browser);
        assert!(!capabilities.file_transfer);
        assert!(!capabilities.task_ack);
    }
}
