pub mod storage;
pub mod ts_adapter;
pub mod validate;
pub mod wal;

// 保留原有 ingest::* 公共接口。
pub use validate::*;
