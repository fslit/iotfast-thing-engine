pub mod catalog;
pub mod core;
pub mod ingest;
pub mod query;
pub mod server;

// 保留原型原有公共路径，避免目录调整影响现有调用方。
pub use catalog::{asset as asset_uns, binding, model as model_catalog, schema, sqlite_store};
pub use core::{config, contract as d00, error, health, scope};
pub use ingest::{storage as storage_duckdb, ts_adapter, wal};
pub use query::{northbound, subscription};
