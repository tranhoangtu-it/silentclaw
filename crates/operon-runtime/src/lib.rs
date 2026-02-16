pub mod runtime;
pub mod storage;
pub mod tool;

pub use runtime::Runtime;
pub use storage::Storage;
pub use tool::Tool;

/// Initialize structured JSON logging
pub fn init_logging() {
    use tracing_subscriber::{fmt, EnvFilter};

    fmt()
        .json()
        .with_env_filter(EnvFilter::from_default_env())
        .init();
}
