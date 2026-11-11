use crate::core::error::{EngineError, ErrorCode};
use duckdb::params;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_double, c_int, c_void};
use std::path::{Path, PathBuf};
use std::ptr;

#[repr(C)]
struct Sqlite3Opaque {
    _private: [u8; 0],
}

#[repr(C)]
struct Sqlite3StmtOpaque {
    _private: [u8; 0],
}

const SQLITE_OK: c_int = 0;
const SQLITE_ROW: c_int = 100;
const SQLITE_DONE: c_int = 101;
const SQLITE_OPEN_READWRITE: c_int = 0x00000002;
const SQLITE_OPEN_CREATE: c_int = 0x00000004;
const SQLITE_OPEN_FULLMUTEX: c_int = 0x00010000;
const SQLITE_TRANSIENT: isize = -1;

#[link(name = "sqlite3")]
extern "C" {
    fn sqlite3_open_v2(
        filename: *const c_char,
        pp_db: *mut *mut Sqlite3Opaque,
        flags: c_int,
        z_vfs: *const c_char,
    ) -> c_int;
    fn sqlite3_close_v2(db: *mut Sqlite3Opaque) -> c_int;
    fn sqlite3_errmsg(db: *mut Sqlite3Opaque) -> *const c_char;
    fn sqlite3_exec(
        db: *mut Sqlite3Opaque,
        sql: *const c_char,
        callback: Option<
             unsafe extern "C" fn(*mut c_void, c_int, *mut *mut c_char, *mut *mut c_char) -> c_int,
        >,
        arg: *mut c_void,
        errmsg: *mut *mut c_char,
    ) -> c_int;
    fn sqlite3_free(ptr: *mut c_void);
    fn sqlite3_prepare_v2(
        db: *mut Sqlite3Opaque,
        z_sql: *const c_char,
        n_byte: c_int,
        pp_stmt: *mut *mut Sqlite3StmtOpaque,
        pz_tail: *mut *const c_char,
    ) -> c_int;
    fn sqlite3_step(stmt: *mut Sqlite3StmtOpaque) -> c_int;
    fn sqlite3_finalize(stmt: *mut Sqlite3StmtOpaque) -> c_int;
    fn sqlite3_bind_text(
        stmt: *mut Sqlite3StmtOpaque,
        idx: c_int,
        val: *const c_char,
        n_bytes: c_int,
        destructor: *const c_void,
    ) -> c_int;
    fn sqlite3_bind_int64(stmt: *mut Sqlite3StmtOpaque, idx: c_int, val: i64) -> c_int;
    fn sqlite3_bind_double(stmt: *mut Sqlite3StmtOpaque, idx: c_int, val: c_double) -> c_int;
    fn sqlite3_bind_null(stmt: *mut Sqlite3StmtOpaque, idx: c_int) -> c_int;
    fn sqlite3_column_type(stmt: *mut Sqlite3StmtOpaque, i_col: c_int) -> c_int;
    fn sqlite3_column_text(stmt: *mut Sqlite3StmtOpaque, i_col: c_int) -> *const u8;
    fn sqlite3_column_int64(stmt: *mut Sqlite3StmtOpaque, i_col: c_int) -> i64;
    fn sqlite3_column_double(stmt: *mut Sqlite3StmtOpaque, i_col: c_int) -> c_double;
    fn sqlite3_libversion() -> *const c_char;
}

const SQLITE_NULL: c_int = 5;

pub const BUSINESS_METADATA_TABLES: &[&str] = &[
    "product",
    "model_revision",
    "model_item",
    "asset",
    "device",
    "resource",
    "resource_path",
    "source_gateway",
    "source_device_grant",
    "binding_revision",
    "point_binding",
    "asset_relation",
    "edge_adapter",
    "derived_metric",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BusinessStoreStatus {
    pub engine: String,
    pub sqlite_version: String,
    pub db_path: String,
    pub journal_mode: String,
    pub tables: Vec<String>,
    pub row_counts: BTreeMap<String, usize>,
}

/// SQLite-backed OLTP persistence store for business/control-plane metadata
/// (`product`, `model_revision`, `model_item`, `asset`, `device`, `resource`,
/// `resource_path`, `source_gateway`, `source_device_grant`, `binding_revision`,
/// `point_binding`, `asset_relation`).
///
/// Device time-series data (`latest_value`, `history_observation`, etc.) is NOT stored here;
/// it is routed to the `TimeSeriesStorageAdapter` (DuckDB by default, extensible to TDengine).
pub struct SqliteBusinessStore {
    db_path: PathBuf,
    raw: *mut Sqlite3Opaque,
}

unsafe impl Send for SqliteBusinessStore {}
unsafe impl Sync for SqliteBusinessStore {}

impl Drop for SqliteBusinessStore {
    fn drop(&mut self) {
        if !self.raw.is_null() {
            unsafe {
                sqlite3_close_v2(self.raw);
            }
            self.raw = ptr::null_mut();
        }
    }
}

impl SqliteBusinessStore {
    pub fn open(path: &Path) -> Result<Self, EngineError> {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let path_str = path.to_string_lossy().to_string();
        let c_path = CString::new(path_str.as_bytes()).map_err(|e| {
            EngineError::new(
                "req-sqlite-open",
                ErrorCode::InternalError,
                format!("Invalid SQLite path: {}", e),
                vec![],
            )
        })?;
        let mut db: *mut Sqlite3Opaque = ptr::null_mut();
        let flags = SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE | SQLITE_OPEN_FULLMUTEX;
        let rc = unsafe { sqlite3_open_v2(c_path.as_ptr(), &mut db, flags, ptr::null()) };
        if rc != SQLITE_OK || db.is_null() {
            let msg = if !db.is_null() {
                unsafe {
                    let p = sqlite3_errmsg(db);
                    let s = if p.is_null() {
                        "unknown sqlite open error".to_string()
                    } else {
                        CStr::from_ptr(p).to_string_lossy().into_owned()
                    };
                    sqlite3_close_v2(db);
                    s
                }
            } else {
                format!("sqlite3_open_v2 failed with code {}", rc)
            };
            return Err(EngineError::new(
                "req-sqlite-open",
                ErrorCode::InternalError,
                msg,
                vec![],
            ));
        }

        let store = Self {
            db_path: path.to_path_buf(),
            raw: db,
        };
        store.init_schema()?;
        Ok(store)
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn sqlite_version() -> String {
        unsafe {
            let p = sqlite3_libversion();
            if p.is_null() {
                "3.x".to_string()
            } else {
                CStr::from_ptr(p).to_string_lossy().into_owned()
            }
        }
    }

    pub fn exec_batch(&self, sql: &str) -> Result<(), EngineError> {
        let c_sql = CString::new(sql).map_err(|e| {
            EngineError::new(
                "req-sqlite-exec",
                ErrorCode::InternalError,
                format!("SQLite SQL contains interior NUL: {}", e),
                vec![],
            )
        })?;
        let mut errmsg: *mut c_char = ptr::null_mut();
        let rc = unsafe {
            sqlite3_exec(
                self.raw,
                c_sql.as_ptr(),
                None,
                ptr::null_mut(),
                &mut errmsg,
            )
        };
        if rc != SQLITE_OK {
            let err_str = unsafe {
                if !errmsg.is_null() {
                    let s = CStr::from_ptr(errmsg).to_string_lossy().into_owned();
                    sqlite3_free(errmsg as *mut c_void);
                    s
                } else {
                    let p = sqlite3_errmsg(self.raw);
                    if p.is_null() {
                        format!("sqlite3_exec error {}", rc)
                    } else {
                        CStr::from_ptr(p).to_string_lossy().into_owned()
                    }
                }
            };
            return Err(EngineError::new(
                "req-sqlite-exec",
                ErrorCode::InternalError,
                format!("SQLite exec failed: {}", err_str),
                vec![],
            ));
        }
        Ok(())
    }

    pub fn vacuum_into(&self, dest_path: &Path) -> Result<(), EngineError> {
        if let Some(parent) = dest_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if dest_path.exists() {
            let _ = std::fs::remove_file(dest_path);
        }
        let escaped = dest_path.to_string_lossy().replace('\'', "''");
        self.exec_batch(&format!("VACUUM INTO '{}';", escaped))
    }

    pub fn init_schema(&self) -> Result<(), EngineError> {
        self.exec_batch(
            r#"
            PRAGMA journal_mode=WAL;
            PRAGMA synchronous=NORMAL;
            PRAGMA foreign_keys=ON;

            CREATE TABLE IF NOT EXISTS product (
                product_id TEXT PRIMARY KEY,
                tenant_id TEXT NOT NULL,
                product_key TEXT NOT NULL,
                name TEXT NOT NULL,
                created_at_ms INTEGER NOT NULL,
                UNIQUE (tenant_id, product_key)
            );

            CREATE TABLE IF NOT EXISTS model_revision (
                model_revision_id TEXT PRIMARY KEY,
                tenant_id TEXT NOT NULL,
                product_id TEXT NOT NULL,
                model_version TEXT NOT NULL,
                status TEXT NOT NULL,
                model_hash TEXT NOT NULL,
                published_at_ms INTEGER,
                UNIQUE (tenant_id, product_id, model_version)
            );

            CREATE TABLE IF NOT EXISTS model_item (
                model_revision_id TEXT NOT NULL,
                identifier TEXT NOT NULL,
                name TEXT NOT NULL,
                value_type TEXT NOT NULL,
                unit_code TEXT,
                unit_symbol TEXT,
                min_value REAL,
                max_value REAL,
                required INTEGER NOT NULL,
                PRIMARY KEY (model_revision_id, identifier)
            );

            CREATE TABLE IF NOT EXISTS asset (
                asset_id TEXT PRIMARY KEY,
                tenant_id TEXT NOT NULL,
                parent_asset_id TEXT,
                slug TEXT NOT NULL,
                name TEXT NOT NULL,
                asset_path TEXT NOT NULL,
                UNIQUE (tenant_id, asset_path)
            );

            CREATE TABLE IF NOT EXISTS device (
                device_id TEXT PRIMARY KEY,
                tenant_id TEXT NOT NULL,
                device_key TEXT NOT NULL,
                device_name TEXT NOT NULL,
                product_id TEXT NOT NULL,
                model_revision_id TEXT NOT NULL,
                active_binding_revision_id TEXT,
                asset_id TEXT NOT NULL,
                canonical_device_uri TEXT NOT NULL,
                revision INTEGER NOT NULL,
                UNIQUE (tenant_id, device_key)
            );

            CREATE TABLE IF NOT EXISTS resource (
                resource_id TEXT PRIMARY KEY,
                tenant_id TEXT NOT NULL,
                device_id TEXT NOT NULL,
                property TEXT NOT NULL,
                name TEXT NOT NULL,
                canonical_uri TEXT NOT NULL,
                UNIQUE (device_id, property)
            );

            CREATE TABLE IF NOT EXISTS resource_path (
                uri TEXT PRIMARY KEY,
                tenant_id TEXT NOT NULL,
                resource_id TEXT NOT NULL,
                path_state TEXT NOT NULL,
                redirect_to_uri TEXT,
                redirect_until_ms INTEGER
            );

            CREATE TABLE IF NOT EXISTS source_gateway (
                tenant_id TEXT NOT NULL,
                source_id TEXT NOT NULL,
                status TEXT NOT NULL,
                connection_state TEXT NOT NULL,
                PRIMARY KEY (tenant_id, source_id)
            );

            CREATE TABLE IF NOT EXISTS source_device_grant (
                tenant_id TEXT NOT NULL,
                source_id TEXT NOT NULL,
                device_id TEXT NOT NULL,
                valid_from_ms INTEGER NOT NULL,
                valid_to_ms INTEGER,
                PRIMARY KEY (tenant_id, source_id, device_id)
            );

            CREATE TABLE IF NOT EXISTS binding_revision (
                binding_revision_id TEXT PRIMARY KEY,
                tenant_id TEXT NOT NULL,
                device_id TEXT NOT NULL,
                model_revision_id TEXT NOT NULL,
                source_id TEXT NOT NULL,
                normalization_owner TEXT NOT NULL,
                server_scale_factor REAL,
                status TEXT NOT NULL,
                config_hash TEXT NOT NULL,
                valid_from_ms INTEGER,
                valid_to_ms INTEGER
            );

            CREATE TABLE IF NOT EXISTS point_binding (
                binding_revision_id TEXT NOT NULL,
                property TEXT NOT NULL,
                source_kind TEXT NOT NULL,
                register_address TEXT NOT NULL,
                factor REAL NOT NULL,
                unit_code TEXT,
                deadband REAL,
                heartbeat_ms INTEGER,
                static_value_json TEXT,
                PRIMARY KEY (binding_revision_id, property)
            );

            CREATE TABLE IF NOT EXISTS asset_relation (
                relation_id TEXT PRIMARY KEY,
                tenant_id TEXT NOT NULL,
                from_asset_id TEXT NOT NULL,
                relation_type TEXT NOT NULL,
                to_asset_id TEXT NOT NULL,
                valid_from_ms INTEGER NOT NULL,
                valid_to_ms INTEGER,
                source TEXT NOT NULL,
                revision INTEGER NOT NULL,
                UNIQUE (tenant_id, from_asset_id, relation_type, to_asset_id)
            );

            CREATE TABLE IF NOT EXISTS edge_adapter (
                adapter_id TEXT PRIMARY KEY,
                tenant_id TEXT NOT NULL,
                protocol TEXT NOT NULL,
                supported_value_types_json TEXT NOT NULL,
                supported_transforms_json TEXT NOT NULL,
                max_batch_size INTEGER NOT NULL,
                supports_deadband INTEGER NOT NULL,
                updated_at_ms INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS derived_metric (
                metric_id TEXT PRIMARY KEY,
                tenant_id TEXT NOT NULL,
                name TEXT NOT NULL,
                product_id TEXT NOT NULL,
                model_revision_id TEXT NOT NULL,
                expression TEXT NOT NULL,
                input_properties_json TEXT NOT NULL,
                output_unit_code TEXT,
                quality_policy TEXT NOT NULL,
                created_at_ms INTEGER NOT NULL
            );
            "#,
        )
    }

    pub fn count_table_rows(&self, table: &str) -> Result<usize, EngineError> {
        let sql = format!("SELECT COUNT(*) FROM {}", table);
        let c_sql = CString::new(sql).unwrap();
        let mut stmt: *mut Sqlite3StmtOpaque = ptr::null_mut();
        let rc = unsafe {
            sqlite3_prepare_v2(
                self.raw,
                c_sql.as_ptr(),
                -1,
                &mut stmt,
                ptr::null_mut(),
            )
        };
        if rc != SQLITE_OK || stmt.is_null() {
            return Err(EngineError::new(
                "req-sqlite-count",
                ErrorCode::InternalError,
                format!("Failed to prepare count query for {}", table),
                vec![],
            ));
        }
        let mut count = 0usize;
        unsafe {
            if sqlite3_step(stmt) == SQLITE_ROW {
                count = sqlite3_column_int64(stmt, 0) as usize;
            }
            sqlite3_finalize(stmt);
        }
        Ok(count)
    }

    pub fn status_snapshot(&self) -> Result<BusinessStoreStatus, EngineError> {
        let mut counts = BTreeMap::new();
        for &tbl in BUSINESS_METADATA_TABLES {
            counts.insert(tbl.to_string(), self.count_table_rows(tbl)?);
        }
        Ok(BusinessStoreStatus {
            engine: "sqlite".to_string(),
            sqlite_version: Self::sqlite_version(),
            db_path: self.db_path.to_string_lossy().to_string(),
            journal_mode: "wal".to_string(),
            tables: BUSINESS_METADATA_TABLES
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            row_counts: counts,
        })
    }

    /// Synchronize business/control-plane metadata from the runtime catalog tables into SQLite.
    /// Device time-series data (`latest_value`, `history_observation`) is excluded.
    pub fn sync_from_catalog(&self, conn: &duckdb::Connection) -> Result<(), EngineError> {
        self.exec_batch("BEGIN IMMEDIATE TRANSACTION;")?;
        let res = (|| -> Result<(), EngineError> {
            for &tbl in BUSINESS_METADATA_TABLES {
                self.exec_batch(&format!("DELETE FROM {};", tbl))?;
            }

            // 1. product
            {
                let mut q = conn.prepare(
                    "SELECT product_id, tenant_id, product_key, name, created_at_ms FROM product",
                )?;
                let mut rows = q.query([])?;
                while let Some(r) = rows.next()? {
                    let p0: String = r.get(0)?;
                    let p1: String = r.get(1)?;
                    let p2: String = r.get(2)?;
                    let p3: String = r.get(3)?;
                    let p4: i64 = r.get(4)?;
                    self.exec_prepared(
                        "INSERT INTO product VALUES (?1, ?2, ?3, ?4, ?5)",
                        &[
                            SqlParam::Text(p0),
                            SqlParam::Text(p1),
                            SqlParam::Text(p2),
                            SqlParam::Text(p3),
                            SqlParam::Int(p4),
                        ],
                    )?;
                }
            }

            // 2. model_revision
            {
                let mut q = conn.prepare(
                    "SELECT model_revision_id, tenant_id, product_id, model_version, status, model_hash, published_at_ms FROM model_revision",
                )?;
                let mut rows = q.query([])?;
                while let Some(r) = rows.next()? {
                    self.exec_prepared(
                        "INSERT INTO model_revision VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                        &[
                            SqlParam::Text(r.get(0)?),
                            SqlParam::Text(r.get(1)?),
                            SqlParam::Text(r.get(2)?),
                            SqlParam::Text(r.get(3)?),
                            SqlParam::Text(r.get(4)?),
                            SqlParam::Text(r.get(5)?),
                            SqlParam::OptInt(r.get(6)?),
                        ],
                    )?;
                }
            }

            // 3. model_item
            {
                let mut q = conn.prepare(
                    "SELECT model_revision_id, identifier, name, value_type, unit_code, unit_symbol, min_value, max_value, required FROM model_item",
                )?;
                let mut rows = q.query([])?;
                while let Some(r) = rows.next()? {
                    let req_b: bool = r.get(8)?;
                    self.exec_prepared(
                        "INSERT INTO model_item VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                        &[
                            SqlParam::Text(r.get(0)?),
                            SqlParam::Text(r.get(1)?),
                            SqlParam::Text(r.get(2)?),
                            SqlParam::Text(r.get(3)?),
                            SqlParam::OptText(r.get(4)?),
                            SqlParam::OptText(r.get(5)?),
                            SqlParam::OptFloat(r.get(6)?),
                            SqlParam::OptFloat(r.get(7)?),
                            SqlParam::Int(if req_b { 1 } else { 0 }),
                        ],
                    )?;
                }
            }

            // 4. asset
            {
                let mut q = conn.prepare(
                    "SELECT asset_id, tenant_id, parent_asset_id, slug, name, asset_path FROM asset",
                )?;
                let mut rows = q.query([])?;
                while let Some(r) = rows.next()? {
                    self.exec_prepared(
                        "INSERT INTO asset VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        &[
                            SqlParam::Text(r.get(0)?),
                            SqlParam::Text(r.get(1)?),
                            SqlParam::OptText(r.get(2)?),
                            SqlParam::Text(r.get(3)?),
                            SqlParam::Text(r.get(4)?),
                            SqlParam::Text(r.get(5)?),
                        ],
                    )?;
                }
            }

            // 5. device
            {
                let mut q = conn.prepare(
                    "SELECT device_id, tenant_id, device_key, device_name, product_id, model_revision_id, active_binding_revision_id, asset_id, canonical_device_uri, revision FROM device",
                )?;
                let mut rows = q.query([])?;
                while let Some(r) = rows.next()? {
                    self.exec_prepared(
                        "INSERT INTO device VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                        &[
                            SqlParam::Text(r.get(0)?),
                            SqlParam::Text(r.get(1)?),
                            SqlParam::Text(r.get(2)?),
                            SqlParam::Text(r.get(3)?),
                            SqlParam::Text(r.get(4)?),
                            SqlParam::Text(r.get(5)?),
                            SqlParam::OptText(r.get(6)?),
                            SqlParam::Text(r.get(7)?),
                            SqlParam::Text(r.get(8)?),
                            SqlParam::Int(r.get(9)?),
                        ],
                    )?;
                }
            }

            // 6. resource
            {
                let mut q = conn.prepare(
                    "SELECT resource_id, tenant_id, device_id, property, name, canonical_uri FROM resource",
                )?;
                let mut rows = q.query([])?;
                while let Some(r) = rows.next()? {
                    self.exec_prepared(
                        "INSERT INTO resource VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        &[
                            SqlParam::Text(r.get(0)?),
                            SqlParam::Text(r.get(1)?),
                            SqlParam::Text(r.get(2)?),
                            SqlParam::Text(r.get(3)?),
                            SqlParam::Text(r.get(4)?),
                            SqlParam::Text(r.get(5)?),
                        ],
                    )?;
                }
            }

            // 7. resource_path
            {
                let mut q = conn.prepare(
                    "SELECT uri, tenant_id, resource_id, path_state, redirect_to_uri, redirect_until_ms FROM resource_path",
                )?;
                let mut rows = q.query([])?;
                while let Some(r) = rows.next()? {
                    self.exec_prepared(
                        "INSERT INTO resource_path VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        &[
                            SqlParam::Text(r.get(0)?),
                            SqlParam::Text(r.get(1)?),
                            SqlParam::Text(r.get(2)?),
                            SqlParam::Text(r.get(3)?),
                            SqlParam::OptText(r.get(4)?),
                            SqlParam::OptInt(r.get(5)?),
                        ],
                    )?;
                }
            }

            // 8. source_gateway
            {
                let mut q = conn.prepare(
                    "SELECT tenant_id, source_id, status, connection_state FROM source_gateway",
                )?;
                let mut rows = q.query([])?;
                while let Some(r) = rows.next()? {
                    self.exec_prepared(
                        "INSERT INTO source_gateway VALUES (?1, ?2, ?3, ?4)",
                        &[
                            SqlParam::Text(r.get(0)?),
                            SqlParam::Text(r.get(1)?),
                            SqlParam::Text(r.get(2)?),
                            SqlParam::Text(r.get(3)?),
                        ],
                    )?;
                }
            }

            // 9. source_device_grant
            {
                let mut q = conn.prepare(
                    "SELECT tenant_id, source_id, device_id, valid_from_ms, valid_to_ms FROM source_device_grant",
                )?;
                let mut rows = q.query([])?;
                while let Some(r) = rows.next()? {
                    self.exec_prepared(
                        "INSERT INTO source_device_grant VALUES (?1, ?2, ?3, ?4, ?5)",
                        &[
                            SqlParam::Text(r.get(0)?),
                            SqlParam::Text(r.get(1)?),
                            SqlParam::Text(r.get(2)?),
                            SqlParam::Int(r.get(3)?),
                            SqlParam::OptInt(r.get(4)?),
                        ],
                    )?;
                }
            }

            // 10. binding_revision
            {
                let mut q = conn.prepare(
                    "SELECT binding_revision_id, tenant_id, device_id, model_revision_id, source_id, normalization_owner, server_scale_factor, status, config_hash, valid_from_ms, valid_to_ms FROM binding_revision",
                )?;
                let mut rows = q.query([])?;
                while let Some(r) = rows.next()? {
                    self.exec_prepared(
                        "INSERT INTO binding_revision VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                        &[
                            SqlParam::Text(r.get(0)?),
                            SqlParam::Text(r.get(1)?),
                            SqlParam::Text(r.get(2)?),
                            SqlParam::Text(r.get(3)?),
                            SqlParam::Text(r.get(4)?),
                            SqlParam::Text(r.get(5)?),
                            SqlParam::OptFloat(r.get(6)?),
                            SqlParam::Text(r.get(7)?),
                            SqlParam::Text(r.get(8)?),
                            SqlParam::OptInt(r.get(9)?),
                            SqlParam::OptInt(r.get(10)?),
                        ],
                    )?;
                }
            }

            // 11. point_binding
            {
                let mut q = conn.prepare(
                    "SELECT binding_revision_id, property, source_kind, register_address, factor, unit_code, deadband, heartbeat_ms, static_value_json FROM point_binding",
                )?;
                let mut rows = q.query([])?;
                while let Some(r) = rows.next()? {
                    self.exec_prepared(
                        "INSERT INTO point_binding VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                        &[
                            SqlParam::Text(r.get(0)?),
                            SqlParam::Text(r.get(1)?),
                            SqlParam::Text(r.get(2)?),
                            SqlParam::Text(r.get(3)?),
                            SqlParam::Float(r.get(4)?),
                            SqlParam::OptText(r.get(5)?),
                            SqlParam::OptFloat(r.get(6)?),
                            SqlParam::OptInt(r.get(7)?),
                            SqlParam::OptText(r.get(8)?),
                        ],
                    )?;
                }
            }

            // 12. asset_relation
            {
                let mut q = conn.prepare(
                    "SELECT relation_id, tenant_id, from_asset_id, relation_type, to_asset_id, valid_from_ms, valid_to_ms, source, revision FROM asset_relation",
                )?;
                let mut rows = q.query([])?;
                while let Some(r) = rows.next()? {
                    self.exec_prepared(
                        "INSERT INTO asset_relation VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                        &[
                            SqlParam::Text(r.get(0)?),
                            SqlParam::Text(r.get(1)?),
                            SqlParam::Text(r.get(2)?),
                            SqlParam::Text(r.get(3)?),
                            SqlParam::Text(r.get(4)?),
                            SqlParam::Int(r.get(5)?),
                            SqlParam::OptInt(r.get(6)?),
                            SqlParam::Text(r.get(7)?),
                            SqlParam::Int(r.get(8)?),
                        ],
                    )?;
                }
            }

            // 13. edge_adapter
            {
                let mut q = conn.prepare(
                    "SELECT adapter_id, tenant_id, protocol, supported_value_types_json, supported_transforms_json, max_batch_size, supports_deadband, updated_at_ms FROM edge_adapter",
                )?;
                let mut rows = q.query([])?;
                while let Some(r) = rows.next()? {
                    let db_bool: bool = r.get(6)?;
                    self.exec_prepared(
                        "INSERT INTO edge_adapter VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                        &[
                            SqlParam::Text(r.get(0)?),
                            SqlParam::Text(r.get(1)?),
                            SqlParam::Text(r.get(2)?),
                            SqlParam::Text(r.get(3)?),
                            SqlParam::Text(r.get(4)?),
                            SqlParam::Int(r.get(5)?),
                            SqlParam::Int(if db_bool { 1 } else { 0 }),
                            SqlParam::Int(r.get(7)?),
                        ],
                    )?;
                }
            }

            // 14. derived_metric
            {
                let mut q = conn.prepare(
                    "SELECT metric_id, tenant_id, name, product_id, model_revision_id, expression, input_properties_json, output_unit_code, quality_policy, created_at_ms FROM derived_metric",
                )?;
                let mut rows = q.query([])?;
                while let Some(r) = rows.next()? {
                    self.exec_prepared(
                        "INSERT INTO derived_metric VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                        &[
                            SqlParam::Text(r.get(0)?),
                            SqlParam::Text(r.get(1)?),
                            SqlParam::Text(r.get(2)?),
                            SqlParam::Text(r.get(3)?),
                            SqlParam::Text(r.get(4)?),
                            SqlParam::Text(r.get(5)?),
                            SqlParam::Text(r.get(6)?),
                            SqlParam::OptText(r.get(7)?),
                            SqlParam::Text(r.get(8)?),
                            SqlParam::Int(r.get(9)?),
                        ],
                    )?;
                }
            }
            Ok(())
        })();

        match res {
            Ok(()) => self.exec_batch("COMMIT;"),
            Err(e) => {
                let _ = self.exec_batch("ROLLBACK;");
                Err(e)
            }
        }
    }

    /// Restore persisted business metadata from SQLite into the runtime catalog tables
    /// if the runtime catalog is empty (e.g. fresh TSDB file or recovery).
    pub fn restore_to_catalog(&self, conn: &duckdb::Connection) -> Result<usize, EngineError> {
        let sqlite_devices = self.count_table_rows("device")?;
        let sqlite_products = self.count_table_rows("product")?;
        let sqlite_adapters = self.count_table_rows("edge_adapter")?;
        let sqlite_metrics = self.count_table_rows("derived_metric")?;
        if sqlite_devices == 0
            && sqlite_products == 0
            && sqlite_adapters == 0
            && sqlite_metrics == 0
        {
            return Ok(0);
        }

        let mut restored = 0usize;
        // Restore product
        for row in self.query_rows("SELECT product_id, tenant_id, product_key, name, created_at_ms FROM product", 5)? {
            conn.execute(
                "INSERT INTO product VALUES (?, ?, ?, ?, ?) ON CONFLICT (product_id) DO NOTHING",
                params![row[0].as_str(), row[1].as_str(), row[2].as_str(), row[3].as_str(), row[4].as_i64()],
            )?;
            restored += 1;
        }
        // Restore model_revision
        for row in self.query_rows("SELECT model_revision_id, tenant_id, product_id, model_version, status, model_hash, published_at_ms FROM model_revision", 7)? {
            conn.execute(
                "INSERT INTO model_revision VALUES (?, ?, ?, ?, ?, ?, ?) ON CONFLICT (model_revision_id) DO NOTHING",
                params![row[0].as_str(), row[1].as_str(), row[2].as_str(), row[3].as_str(), row[4].as_str(), row[5].as_str(), row[6].as_opt_i64()],
            )?;
            restored += 1;
        }
        // Restore model_item
        for row in self.query_rows("SELECT model_revision_id, identifier, name, value_type, unit_code, unit_symbol, min_value, max_value, required FROM model_item", 9)? {
            conn.execute(
                "INSERT INTO model_item VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT (model_revision_id, identifier) DO NOTHING",
                params![
                    row[0].as_str(),
                    row[1].as_str(),
                    row[2].as_str(),
                    row[3].as_str(),
                    row[4].as_opt_str(),
                    row[5].as_opt_str(),
                    row[6].as_opt_f64(),
                    row[7].as_opt_f64(),
                    row[8].as_i64() != 0
                ],
            )?;
            restored += 1;
        }
        // Restore asset
        for row in self.query_rows("SELECT asset_id, tenant_id, parent_asset_id, slug, name, asset_path FROM asset", 6)? {
            conn.execute(
                "INSERT INTO asset VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT (asset_id) DO NOTHING",
                params![row[0].as_str(), row[1].as_str(), row[2].as_opt_str(), row[3].as_str(), row[4].as_str(), row[5].as_str()],
            )?;
            restored += 1;
        }
        // Restore device
        for row in self.query_rows("SELECT device_id, tenant_id, device_key, device_name, product_id, model_revision_id, active_binding_revision_id, asset_id, canonical_device_uri, revision FROM device", 10)? {
            conn.execute(
                "INSERT INTO device VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT (device_id) DO NOTHING",
                params![
                    row[0].as_str(),
                    row[1].as_str(),
                    row[2].as_str(),
                    row[3].as_str(),
                    row[4].as_str(),
                    row[5].as_str(),
                    row[6].as_opt_str(),
                    row[7].as_str(),
                    row[8].as_str(),
                    row[9].as_i64()
                ],
            )?;
            restored += 1;
        }
        // Restore resource
        for row in self.query_rows("SELECT resource_id, tenant_id, device_id, property, name, canonical_uri FROM resource", 6)? {
            conn.execute(
                "INSERT INTO resource VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT (resource_id) DO NOTHING",
                params![row[0].as_str(), row[1].as_str(), row[2].as_str(), row[3].as_str(), row[4].as_str(), row[5].as_str()],
            )?;
            restored += 1;
        }
        // Restore resource_path
        for row in self.query_rows("SELECT uri, tenant_id, resource_id, path_state, redirect_to_uri, redirect_until_ms FROM resource_path", 6)? {
            conn.execute(
                "INSERT INTO resource_path VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT (uri) DO NOTHING",
                params![row[0].as_str(), row[1].as_str(), row[2].as_str(), row[3].as_str(), row[4].as_opt_str(), row[5].as_opt_i64()],
            )?;
            restored += 1;
        }
        // Restore source_gateway
        for row in self.query_rows("SELECT tenant_id, source_id, status, connection_state FROM source_gateway", 4)? {
            conn.execute(
                "INSERT INTO source_gateway VALUES (?, ?, ?, ?) ON CONFLICT (tenant_id, source_id) DO NOTHING",
                params![row[0].as_str(), row[1].as_str(), row[2].as_str(), row[3].as_str()],
            )?;
            restored += 1;
        }
        // Restore source_device_grant
        for row in self.query_rows("SELECT tenant_id, source_id, device_id, valid_from_ms, valid_to_ms FROM source_device_grant", 5)? {
            conn.execute(
                "INSERT INTO source_device_grant VALUES (?, ?, ?, ?, ?) ON CONFLICT (tenant_id, source_id, device_id) DO NOTHING",
                params![row[0].as_str(), row[1].as_str(), row[2].as_str(), row[3].as_i64(), row[4].as_opt_i64()],
            )?;
            restored += 1;
        }
        // Restore binding_revision
        for row in self.query_rows("SELECT binding_revision_id, tenant_id, device_id, model_revision_id, source_id, normalization_owner, server_scale_factor, status, config_hash, valid_from_ms, valid_to_ms FROM binding_revision", 11)? {
            conn.execute(
                "INSERT INTO binding_revision VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT (binding_revision_id) DO NOTHING",
                params![
                    row[0].as_str(),
                    row[1].as_str(),
                    row[2].as_str(),
                    row[3].as_str(),
                    row[4].as_str(),
                    row[5].as_str(),
                    row[6].as_opt_f64(),
                    row[7].as_str(),
                    row[8].as_str(),
                    row[9].as_opt_i64(),
                    row[10].as_opt_i64()
                ],
            )?;
            restored += 1;
        }
        // Restore point_binding
        for row in self.query_rows("SELECT binding_revision_id, property, source_kind, register_address, factor, unit_code, deadband, heartbeat_ms, static_value_json FROM point_binding", 9)? {
            conn.execute(
                "INSERT INTO point_binding VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT (binding_revision_id, property) DO NOTHING",
                params![
                    row[0].as_str(),
                    row[1].as_str(),
                    row[2].as_str(),
                    row[3].as_str(),
                    row[4].as_f64(),
                    row[5].as_opt_str(),
                    row[6].as_opt_f64(),
                    row[7].as_opt_i64(),
                    row[8].as_opt_str()
                ],
            )?;
            restored += 1;
        }
        // Restore asset_relation
        for row in self.query_rows("SELECT relation_id, tenant_id, from_asset_id, relation_type, to_asset_id, valid_from_ms, valid_to_ms, source, revision FROM asset_relation", 9)? {
            conn.execute(
                "INSERT INTO asset_relation VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT (relation_id) DO NOTHING",
                params![
                    row[0].as_str(),
                    row[1].as_str(),
                    row[2].as_str(),
                    row[3].as_str(),
                    row[4].as_str(),
                    row[5].as_i64(),
                    row[6].as_opt_i64(),
                    row[7].as_str(),
                    row[8].as_i64()
                ],
            )?;
            restored += 1;
        }
        // Restore edge_adapter
        for row in self.query_rows("SELECT adapter_id, tenant_id, protocol, supported_value_types_json, supported_transforms_json, max_batch_size, supports_deadband, updated_at_ms FROM edge_adapter", 8)? {
            conn.execute(
                "INSERT INTO edge_adapter VALUES (?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT (adapter_id) DO NOTHING",
                params![
                    row[0].as_str(),
                    row[1].as_str(),
                    row[2].as_str(),
                    row[3].as_str(),
                    row[4].as_str(),
                    row[5].as_i64(),
                    row[6].as_i64() != 0,
                    row[7].as_i64()
                ],
            )?;
            restored += 1;
        }
        // Restore derived_metric
        for row in self.query_rows("SELECT metric_id, tenant_id, name, product_id, model_revision_id, expression, input_properties_json, output_unit_code, quality_policy, created_at_ms FROM derived_metric", 10)? {
            conn.execute(
                "INSERT INTO derived_metric VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT (metric_id) DO NOTHING",
                params![
                    row[0].as_str(),
                    row[1].as_str(),
                    row[2].as_str(),
                    row[3].as_str(),
                    row[4].as_str(),
                    row[5].as_str(),
                    row[6].as_str(),
                    row[7].as_opt_str(),
                    row[8].as_str(),
                    row[9].as_i64()
                ],
            )?;
            restored += 1;
        }

        Ok(restored)
    }

    fn exec_prepared(&self, sql: &str, params: &[SqlParam]) -> Result<(), EngineError> {
        let c_sql = CString::new(sql).unwrap();
        let mut stmt: *mut Sqlite3StmtOpaque = ptr::null_mut();
        let rc = unsafe {
            sqlite3_prepare_v2(
                self.raw,
                c_sql.as_ptr(),
                -1,
                &mut stmt,
                ptr::null_mut(),
            )
        };
        if rc != SQLITE_OK || stmt.is_null() {
            return Err(EngineError::new(
                "req-sqlite-prep",
                ErrorCode::InternalError,
                format!("Failed to prepare SQLite statement: {}", sql),
                vec![],
            ));
        }

        unsafe {
            for (i, param) in params.iter().enumerate() {
                let idx = (i + 1) as c_int;
                match param {
                    SqlParam::Text(s) => {
                        let cs = CString::new(s.as_bytes()).unwrap_or_default();
                        sqlite3_bind_text(
                            stmt,
                            idx,
                            cs.as_ptr(),
                            -1,
                            SQLITE_TRANSIENT as *const c_void,
                        );
                    }
                    SqlParam::OptText(Some(s)) => {
                        let cs = CString::new(s.as_bytes()).unwrap_or_default();
                        sqlite3_bind_text(
                            stmt,
                            idx,
                            cs.as_ptr(),
                            -1,
                            SQLITE_TRANSIENT as *const c_void,
                        );
                    }
                    SqlParam::OptText(None) => {
                        sqlite3_bind_null(stmt, idx);
                    }
                    SqlParam::Int(v) => {
                        sqlite3_bind_int64(stmt, idx, *v);
                    }
                    SqlParam::OptInt(Some(v)) => {
                        sqlite3_bind_int64(stmt, idx, *v);
                    }
                    SqlParam::OptInt(None) => {
                        sqlite3_bind_null(stmt, idx);
                    }
                    SqlParam::Float(v) => {
                        sqlite3_bind_double(stmt, idx, *v);
                    }
                    SqlParam::OptFloat(Some(v)) => {
                        sqlite3_bind_double(stmt, idx, *v);
                    }
                    SqlParam::OptFloat(None) => {
                        sqlite3_bind_null(stmt, idx);
                    }
                }
            }
            let step_rc = sqlite3_step(stmt);
            sqlite3_finalize(stmt);
            if step_rc != SQLITE_DONE && step_rc != SQLITE_ROW {
                return Err(EngineError::new(
                    "req-sqlite-step",
                    ErrorCode::InternalError,
                    format!("SQLite step failed with code {} for {}", step_rc, sql),
                    vec![],
                ));
            }
        }
        Ok(())
    }

    fn query_rows(&self, sql: &str, col_count: usize) -> Result<Vec<Vec<SqlColVal>>, EngineError> {
        let c_sql = CString::new(sql).unwrap();
        let mut stmt: *mut Sqlite3StmtOpaque = ptr::null_mut();
        let rc = unsafe {
            sqlite3_prepare_v2(
                self.raw,
                c_sql.as_ptr(),
                -1,
                &mut stmt,
                ptr::null_mut(),
            )
        };
        if rc != SQLITE_OK || stmt.is_null() {
            return Err(EngineError::new(
                "req-sqlite-query",
                ErrorCode::InternalError,
                format!("Failed to prepare SQLite query: {}", sql),
                vec![],
            ));
        }

        let mut rows = Vec::new();
        unsafe {
            while sqlite3_step(stmt) == SQLITE_ROW {
                let mut cols = Vec::with_capacity(col_count);
                for c in 0..col_count {
                    let c_idx = c as c_int;
                    let col_type = sqlite3_column_type(stmt, c_idx);
                    if col_type == SQLITE_NULL {
                        cols.push(SqlColVal::Null);
                    } else {
                        let ptr = sqlite3_column_text(stmt, c_idx);
                        let text = if ptr.is_null() {
                            String::new()
                        } else {
                            CStr::from_ptr(ptr as *const c_char)
                                .to_string_lossy()
                                .into_owned()
                        };
                        let i_val = sqlite3_column_int64(stmt, c_idx);
                        let f_val = sqlite3_column_double(stmt, c_idx);
                        cols.push(SqlColVal::Present {
                            text,
                            i_val,
                            f_val,
                        });
                    }
                }
                rows.push(cols);
            }
            sqlite3_finalize(stmt);
        }
        Ok(rows)
    }
}

enum SqlParam {
    Text(String),
    OptText(Option<String>),
    Int(i64),
    OptInt(Option<i64>),
    Float(f64),
    OptFloat(Option<f64>),
}

enum SqlColVal {
    Null,
    Present {
        text: String,
        i_val: i64,
        f_val: f64,
    },
}

impl SqlColVal {
    fn as_str(&self) -> String {
        match self {
            SqlColVal::Null => String::new(),
            SqlColVal::Present { text, .. } => text.clone(),
        }
    }

    fn as_opt_str(&self) -> Option<String> {
        match self {
            SqlColVal::Null => None,
            SqlColVal::Present { text, .. } => Some(text.clone()),
        }
    }

    fn as_i64(&self) -> i64 {
        match self {
            SqlColVal::Null => 0,
            SqlColVal::Present { i_val, .. } => *i_val,
        }
    }

    fn as_opt_i64(&self) -> Option<i64> {
        match self {
            SqlColVal::Null => None,
            SqlColVal::Present { i_val, .. } => Some(*i_val),
        }
    }

    fn as_f64(&self) -> f64 {
        match self {
            SqlColVal::Null => 0.0,
            SqlColVal::Present { f_val, .. } => *f_val,
        }
    }

    fn as_opt_f64(&self) -> Option<f64> {
        match self {
            SqlColVal::Null => None,
            SqlColVal::Present { f_val, .. } => Some(*f_val),
        }
    }
}
