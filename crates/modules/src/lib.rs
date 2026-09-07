//! Post-exploitation module interface and registry.
//!
//! A module is a named, self-contained unit of post-exploitation logic that
//! runs on the implant side and reports structured results back through the
//! existing task/Result envelope. Each module maps to an `nw/<module>` command
//! dispatched by the server's task queue.
//!
//! The registry is the single source of truth for which modules exist, their
//! arguments, and their output schema — so the console, the server (for authz),
//! and the implant (for local command dispatch) all agree on what is available.

pub mod automation;
pub mod error;
pub mod module;
pub mod registry;

pub use automation::{Script, ScriptError, Step, StepResult};
pub use error::ModuleError;
pub use module::{Module, ModuleOutput, ModuleResult, Role};
pub use registry::{Registry, SharedRegistry};
