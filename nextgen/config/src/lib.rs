mod inherited_tasks_config;
mod language_platform;
mod project;
mod project_config;
mod relative_path;
mod template;
mod template_config;
mod toolchain;
mod toolchain_config;
mod validate;
mod workspace;
mod workspace_config;

pub use inherited_tasks_config::*;
pub use language_platform::*;
pub use project::*;
pub use project_config::*;
pub use relative_path::*;
pub use template::*;
pub use template_config::*;
pub use toolchain::*;
pub use toolchain_config::*;
pub use workspace::*;
pub use workspace_config::*;

pub use schematic::ConfigError;