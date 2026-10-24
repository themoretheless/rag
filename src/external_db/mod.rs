//! Explicit owner-configured, read-only remote schema captures. Never executes caller SQL.
pub(crate) mod check_expr;
mod compare;
mod marks;
mod metadata;
mod relations;
mod samples;
mod types;
pub mod write_check;
use crate::{
    error::{AppError, Result},
    Store,
};
use futures_util::TryStreamExt;
use rmcp::schemars;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{ConnectOptions, Connection, Executor, Row};
use std::{collections::BTreeMap, str::FromStr, time::Duration};

pub const DDL: &str = "CREATE SEQUENCE IF NOT EXISTS external_db_snapshot_seq; CREATE TABLE IF NOT EXISTS external_db_snapshots (id VARCHAR PRIMARY KEY, connection_alias VARCHAR NOT NULL, captured_ms BIGINT NOT NULL, payload VARCHAR NOT NULL, seq BIGINT DEFAULT nextval('external_db_snapshot_seq') UNIQUE);";

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Target {
    dialect: Dialect,
    url_env: String,
    project: String,
    schemas: Vec<String>,
    #[serde(default)]
    tls: Tls,
    #[serde(default)]
    samples_enabled: bool,
    #[serde(default)]
    sample_columns: BTreeMap<String, BTreeMap<String, Vec<String>>>,
}
#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Dialect {
    Postgresql,
    Mysql,
}
#[derive(Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Tls {
    #[default]
    VerifyFull,
    Disable,
}
fn config_error() -> AppError {
    AppError::config("invalid RAG_EXTERNAL_DATABASES configuration (expected named PostgreSQL/MySQL targets, url_env, project and explicit schemas)")
}
fn targets(raw: &str) -> Result<BTreeMap<String, Target>> {
    if raw.len() > 32768 {
        return Err(config_error());
    }
    let targets: BTreeMap<String, Target> =
        serde_json::from_str(raw).map_err(|_| config_error())?;
    if targets.len() > 16 {
        return Err(config_error());
    }
    for (alias, t) in &targets {
        if alias.is_empty()
            || alias.len() > 64
            || !alias
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
            || t.url_env.is_empty()
            || t.url_env.len() > 128
            || !t
                .url_env
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_')
            || t.project.trim().is_empty()
            || t.project.len() > 200
            || t.schemas.is_empty()
            || t.schemas.len() > 16
            || t.schemas.iter().any(|s| {
                s.is_empty()
                    || s.len() > 128
                    || s.contains('\0')
                    || matches!(
                        s.as_str(),
                        "information_schema"
                            | "pg_catalog"
                            | "mysql"
                            | "sys"
                            | "performance_schema"
                    )
            })
        {
            return Err(config_error());
        }
        if t.sample_columns.len() > 16
            || t.sample_columns.iter().any(|(schema, tables)| {
                !t.schemas.contains(schema)
                    || tables.len() > 200
                    || tables.iter().any(|(name, cols)| {
                        name.is_empty()
                            || name.len() > 128
                            || name.contains('\0')
                            || cols.len() > 16
                            || cols
                                .iter()
                                .any(|c| c.is_empty() || c.len() > 128 || c.contains('\0'))
                    })
            })
        {
            return Err(config_error());
        }
        let mut sorted = t.schemas.clone();
        sorted.sort();
        sorted.dedup();
        if sorted.len() != t.schemas.len() {
            return Err(config_error());
        }
    }
    Ok(targets)
}

fn legacy_capture_format() -> u32 {
    1
}
pub const CAPTURE_FORMAT: u32 = 4;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Snapshot {
    #[serde(default = "legacy_capture_format")]
    pub capture_format: u32,
    pub id: String,
    pub connection: String,
    pub project: String,
    pub dialect: String,
    pub schemas: Vec<String>,
    pub captured_ms: i64,
    pub code_index_revision: Option<i64>,
    pub target_fingerprint: String,
    pub tables: Vec<LiveTable>,
    pub complete_within_visible_scope: bool,
    pub diagnostics: Vec<String>,
    #[serde(default)]
    pub inferred_relations: Option<relations::Inference>,
}
impl Snapshot {
    fn structural_format(&self) -> u32 {
        if self.capture_format <= 1 {
            1
        } else if self.capture_format <= 3 {
            2
        } else {
            3
        }
    }
    fn validate_format(&self) -> Result<()> {
        if !(1..=CAPTURE_FORMAT).contains(&self.capture_format)
            || self.capture_format >= 2 && self.tables.iter().any(|t| t.metadata.is_none())
            || self.capture_format >= 3 && self.inferred_relations.is_none()
            || self.capture_format >= 4
                && self
                    .tables
                    .iter()
                    .any(|t| t.metadata.as_ref().is_none_or(|m| m.types.is_none()))
        {
            return Err(AppError::config("unsupported or incomplete external snapshot format; retain the snapshot and refresh with a compatible server"));
        }
        Ok(())
    }
}
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LiveTable {
    pub schema: String,
    pub name: String,
    pub kind: String,
    pub columns: Vec<Value>,
    pub foreign_keys: Vec<Value>,
    pub complete: bool,
    pub sample: Value,
    #[serde(default)]
    pub metadata: Option<metadata::TableMetadata>,
}
#[derive(Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Query {
    /// Owner-configured connection alias; never a URL.
    pub connection: String,
    pub snapshot_id: Option<String>,
    pub base_snapshot_id: Option<String>,
    /// Exact schema and table names, not SQL identifiers or expressions.
    pub schema: Option<String>,
    pub table: Option<String>,
    pub limit: Option<usize>,
    /// db_relations: all (default), declared or inferred. Arrays remain separate.
    pub relation_kind: Option<String>,
}
#[derive(Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Refresh {
    pub connection: String,
    #[serde(default)]
    pub include_samples: bool,
}

enum Remote {
    Pg(sqlx::PgConnection),
    My(sqlx::MySqlConnection),
}
impl Remote {
    async fn rows(
        &mut self,
        pg: &'static str,
        my: &'static str,
        binds: &[&str],
        fields: &[&str],
    ) -> Result<Vec<Value>> {
        // Every SELECT column is explicitly textual; values remain distinct from NULL.
        match self {
            Self::Pg(c) => {
                let mut q = sqlx::query(pg);
                for b in binds {
                    q = q.bind(*b);
                }
                let mut stream = q.fetch(c);
                let mut rows = vec![];
                let mut received = 0usize;
                while let Some(r) = stream.try_next().await.map_err(|_| remote_error())? {
                    let mut v = serde_json::Map::new();
                    for (i, k) in fields.iter().enumerate() {
                        let value: Option<String> = r.try_get(i).map_err(|_| remote_error())?;
                        received += value.as_ref().map_or(0, String::len);
                        if received > 8 * 1024 * 1024 {
                            return Err(AppError::config("external metadata query exceeded 8 MiB text budget; previous snapshot retained"));
                        }
                        v.insert((*k).into(), json!(value));
                    }
                    rows.push(Value::Object(v));
                }
                Ok(rows)
            }
            Self::My(c) => {
                let mut q = sqlx::query(my);
                for b in binds {
                    q = q.bind(*b);
                }
                let mut stream = q.fetch(c);
                let mut rows = vec![];
                let mut received = 0usize;
                while let Some(r) = stream.try_next().await.map_err(|_| remote_error())? {
                    let mut v = serde_json::Map::new();
                    for (i, k) in fields.iter().enumerate() {
                        let value: Option<String> = r.try_get(i).map_err(|_| remote_error())?;
                        received += value.as_ref().map_or(0, String::len);
                        if received > 8 * 1024 * 1024 {
                            return Err(AppError::config("external metadata query exceeded 8 MiB text budget; previous snapshot retained"));
                        }
                        v.insert((*k).into(), json!(value));
                    }
                    rows.push(Value::Object(v));
                }
                Ok(rows)
            }
        }
    }
    async fn rollback(&mut self) -> Result<()> {
        match self {
            Self::Pg(c) => {
                c.execute("ROLLBACK").await.map_err(|_| remote_error())?;
            }
            Self::My(c) => {
                c.execute("ROLLBACK").await.map_err(|_| remote_error())?;
            }
        }
        Ok(())
    }
}
fn remote_error() -> AppError {
    AppError::config("external database connection or metadata query failed; verify owner credentials, TLS, schema and metadata permissions (driver details suppressed)")
}
fn fingerprint(t: &Target, host: &str, port: u16, user: &str, database: Option<&str>) -> String {
    let mut schemas = t.schemas.clone();
    schemas.sort();
    crate::util::content_hash(
        &json!([t.dialect, t.project, schemas, host, port, user, database]).to_string(),
    )
}
async fn connect(t: &Target, url: &str) -> Result<(Remote, String)> {
    match t.dialect {
        Dialect::Postgresql => {
            let options = sqlx::postgres::PgConnectOptions::from_str(url)
                .map_err(|_| remote_error())?
                .ssl_mode(match t.tls {
                    Tls::VerifyFull => sqlx::postgres::PgSslMode::VerifyFull,
                    Tls::Disable => sqlx::postgres::PgSslMode::Disable,
                })
                .disable_statement_logging();
            let scope = fingerprint(
                t,
                &format!("{}:{:?}", options.get_host(), options.get_socket()),
                options.get_port(),
                options.get_username(),
                options.get_database(),
            );
            let mut c = sqlx::PgConnection::connect_with(&options)
                .await
                .map_err(|_| remote_error())?;
            c.execute("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY")
                .await
                .map_err(|_| remote_error())?;
            c.execute("SET LOCAL search_path = pg_catalog")
                .await
                .map_err(|_| remote_error())?;
            c.execute("SET LOCAL statement_timeout = 10000")
                .await
                .map_err(|_| remote_error())?;
            c.execute("SET LOCAL lock_timeout = 1000")
                .await
                .map_err(|_| remote_error())?;
            Ok((Remote::Pg(c), scope))
        }
        Dialect::Mysql => {
            let options = sqlx::mysql::MySqlConnectOptions::from_str(url)
                .map_err(|_| remote_error())?
                .ssl_mode(match t.tls {
                    Tls::VerifyFull => sqlx::mysql::MySqlSslMode::VerifyIdentity,
                    Tls::Disable => sqlx::mysql::MySqlSslMode::Disabled,
                })
                .disable_statement_logging();
            let scope = fingerprint(
                t,
                &format!("{}:{:?}", options.get_host(), options.get_socket()),
                options.get_port(),
                options.get_username(),
                options.get_database(),
            );
            let mut c = sqlx::MySqlConnection::connect_with(&options)
                .await
                .map_err(|_| remote_error())?;
            c.execute("SET SESSION TRANSACTION ISOLATION LEVEL REPEATABLE READ")
                .await
                .map_err(|_| remote_error())?;
            c.execute("SET SESSION MAX_EXECUTION_TIME = 10000")
                .await
                .map_err(|_| remote_error())?;
            c.execute("START TRANSACTION WITH CONSISTENT SNAPSHOT, READ ONLY")
                .await
                .map_err(|_| remote_error())?;
            Ok((Remote::My(c), scope))
        }
    }
}
async fn collect(
    alias: &str,
    t: &Target,
    url: &str,
    revision: Option<i64>,
    include_samples: bool,
) -> Result<Snapshot> {
    if include_samples && !t.samples_enabled {
        return Err(AppError::config(
            "row samples are disabled by owner configuration",
        ));
    }
    let (mut c, scope) = connect(t, url).await?;
    let mut tables = Vec::new();
    let mut captured_bytes = 0usize;
    let mut diagnostics = Vec::new();
    let mut schemas = t.schemas.clone();
    schemas.sort();
    for schema in &schemas {
        let headers=c.rows("SELECT c.relname::text, CASE c.relkind WHEN 'v' THEN 'VIEW' WHEN 'm' THEN 'MATERIALIZED VIEW' WHEN 'f' THEN 'FOREIGN TABLE' ELSE 'BASE TABLE' END FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND c.relkind IN ('r','p','v','m','f') AND pg_catalog.has_table_privilege(c.oid,'SELECT') ORDER BY c.relname LIMIT 201",
            "SELECT CAST(TABLE_NAME AS CHAR), CAST(TABLE_TYPE AS CHAR) FROM information_schema.TABLES WHERE TABLE_SCHEMA=? ORDER BY TABLE_NAME LIMIT 201",&[schema],&["name","kind"]).await?;
        if headers.len() > 200 {
            diagnostics.push(format!("TABLE_LIMIT:{schema}"));
        }
        for h in headers.into_iter().take(200) {
            if tables.len() >= 200 {
                diagnostics.push("GLOBAL_TABLE_LIMIT".into());
                break;
            }
            let name = h["name"].as_str().ok_or_else(remote_error)?;
            let mut columns=c.rows("SELECT a.attname::text,a.attnum::text,pg_catalog.format_type(a.atttypid,a.atttypmod)::text,tn.nspname::text,t.typname::text,CASE WHEN a.attnotnull OR (t.typtype='d' AND t.typnotnull) THEN 'NO' ELSE 'YES' END,LEFT(CASE WHEN a.attgenerated='' THEN pg_catalog.pg_get_expr(ad.adbin,ad.adrelid) END,2048)::text,CASE WHEN a.attgenerated<>'' THEN 'ALWAYS' ELSE 'NEVER' END,LEFT(CASE WHEN a.attgenerated<>'' THEN pg_catalog.pg_get_expr(ad.adbin,ad.adrelid) END,2048)::text,CASE WHEN a.attidentity<>'' THEN 'YES' ELSE 'NO' END,ic.character_maximum_length::text,ic.numeric_precision::text,ic.numeric_scale::text,ic.datetime_precision::text,(COALESCE(length(pg_catalog.pg_get_expr(ad.adbin,ad.adrelid)),0)>2048)::text,CASE a.attidentity WHEN 'a' THEN 'ALWAYS' WHEN 'd' THEN 'BY DEFAULT' END::text,CASE a.attgenerated WHEN 's' THEN 'STORED' WHEN 'v' THEN 'VIRTUAL' END::text,a.attndims::text,(SELECT cn.nspname::text FROM pg_catalog.pg_collation co JOIN pg_catalog.pg_namespace cn ON cn.oid=co.collnamespace WHERE co.oid=a.attcollation),(SELECT collname::text FROM pg_catalog.pg_collation WHERE oid=a.attcollation) FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace JOIN pg_catalog.pg_attribute a ON a.attrelid=c.oid AND a.attnum>0 AND NOT a.attisdropped JOIN pg_catalog.pg_type t ON t.oid=a.atttypid JOIN pg_catalog.pg_namespace tn ON tn.oid=t.typnamespace LEFT JOIN pg_catalog.pg_attrdef ad ON ad.adrelid=c.oid AND ad.adnum=a.attnum LEFT JOIN information_schema.columns ic ON ic.table_schema=n.nspname AND ic.table_name=c.relname AND ic.column_name=a.attname WHERE n.nspname=$1 AND c.relname=$2 ORDER BY a.attnum LIMIT 257",
                "SELECT CAST(COLUMN_NAME AS CHAR), CAST(ORDINAL_POSITION AS CHAR), LEFT(CAST(COLUMN_TYPE AS CHAR),65536), NULL, NULL, CAST(IS_NULLABLE AS CHAR), LEFT(CAST(COLUMN_DEFAULT AS CHAR),2048), CAST(EXTRA AS CHAR), LEFT(CAST(GENERATION_EXPRESSION AS CHAR),2048), NULL, CAST(CHARACTER_MAXIMUM_LENGTH AS CHAR), CAST(NUMERIC_PRECISION AS CHAR), CAST(NUMERIC_SCALE AS CHAR), CAST(DATETIME_PRECISION AS CHAR), CAST((CHAR_LENGTH(COLUMN_TYPE)>65536 OR COALESCE(CHAR_LENGTH(COLUMN_DEFAULT),0)>2048 OR COALESCE(CHAR_LENGTH(GENERATION_EXPRESSION),0)>2048) AS CHAR),NULL,CASE WHEN EXTRA LIKE '%STORED GENERATED%' THEN 'STORED' WHEN EXTRA LIKE '%VIRTUAL GENERATED%' THEN 'VIRTUAL' END,NULL,NULL,CAST(COLLATION_NAME AS CHAR) FROM information_schema.COLUMNS WHERE TABLE_SCHEMA=? AND TABLE_NAME=? ORDER BY ORDINAL_POSITION LIMIT 257", &[schema,name],
                &["name","ordinal","data_type","type_schema","type_name","nullable","default_sql","generation_metadata","generation_expression","identity","character_maximum_length","numeric_precision","numeric_scale","datetime_precision","metadata_text_truncated","identity_kind","generation_kind","declared_array_dimensions","collation_schema","collation_name"]).await?;
            // Catalog ordinality preserves composite FK pairs even for SELECT-only roles.
            let mut foreign_keys=c.rows("SELECT con.conname::text,a.attname::text,keys.ord::text,tn.nspname::text,tc.relname::text,ta.attname::text,con.confupdtype::text,con.confdeltype::text FROM pg_catalog.pg_constraint con JOIN pg_catalog.pg_class c ON c.oid=con.conrelid JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace JOIN pg_catalog.pg_class tc ON tc.oid=con.confrelid JOIN pg_catalog.pg_namespace tn ON tn.oid=tc.relnamespace CROSS JOIN LATERAL unnest(con.conkey,con.confkey) WITH ORDINALITY AS keys(src,dst,ord) JOIN pg_catalog.pg_attribute a ON a.attrelid=c.oid AND a.attnum=keys.src JOIN pg_catalog.pg_attribute ta ON ta.attrelid=tc.oid AND ta.attnum=keys.dst WHERE con.contype='f' AND n.nspname=$1 AND c.relname=$2 ORDER BY con.conname,keys.ord LIMIT 257",
                "SELECT CAST(k.CONSTRAINT_NAME AS CHAR),CAST(k.COLUMN_NAME AS CHAR),CAST(k.ORDINAL_POSITION AS CHAR),CAST(k.REFERENCED_TABLE_SCHEMA AS CHAR),CAST(k.REFERENCED_TABLE_NAME AS CHAR),CAST(k.REFERENCED_COLUMN_NAME AS CHAR),CAST(r.UPDATE_RULE AS CHAR),CAST(r.DELETE_RULE AS CHAR) FROM information_schema.KEY_COLUMN_USAGE k JOIN information_schema.REFERENTIAL_CONSTRAINTS r ON r.CONSTRAINT_SCHEMA=k.CONSTRAINT_SCHEMA AND r.CONSTRAINT_NAME=k.CONSTRAINT_NAME AND r.TABLE_NAME=k.TABLE_NAME WHERE k.TABLE_SCHEMA=? AND k.TABLE_NAME=? AND k.REFERENCED_TABLE_NAME IS NOT NULL ORDER BY k.CONSTRAINT_NAME,k.ORDINAL_POSITION LIMIT 257",&[schema,name],
                &["constraint","column","ordinal","target_schema","target_table","target_column","on_update","on_delete"]).await?;
            for fk in &mut foreign_keys {
                for action in ["on_update", "on_delete"] {
                    let text = match fk[action].as_str().unwrap_or("") {
                        "a" => Some("NO ACTION"),
                        "r" => Some("RESTRICT"),
                        "c" => Some("CASCADE"),
                        "n" => Some("SET NULL"),
                        "d" => Some("SET DEFAULT"),
                        _ => None,
                    };
                    if let Some(text) = text {
                        fk[action] = json!(text);
                    }
                }
            }
            let text_truncated = columns
                .iter()
                .any(|c| matches!(c["metadata_text_truncated"].as_str(), Some("true" | "1")));
            let kind = h["kind"].as_str().unwrap_or("UNKNOWN");
            let mut metadata = c.metadata(schema, name, kind).await?;
            let types_complete = types::enrich(
                &mut columns,
                metadata.types.as_ref().unwrap(),
                match t.dialect {
                    Dialect::Postgresql => "postgresql",
                    Dialect::Mysql => "mysql",
                },
            );
            if !types_complete {
                metadata.complete = false;
                metadata
                    .diagnostics
                    .push("COLUMN_TYPE_RESOLUTION_INCOMPLETE".into());
            }
            for d in &metadata.diagnostics {
                diagnostics.push(format!("{d}:{schema}/{name}"));
            }
            let complete = metadata.complete
                && columns.len() <= 256
                && foreign_keys.len() <= 256
                && !text_truncated;
            if !complete {
                diagnostics.push(format!("METADATA_LIMIT:{schema}/{name}"));
            }
            let entry = LiveTable {
                schema: schema.clone(),
                name: name.into(),
                kind: h["kind"].as_str().unwrap_or("UNKNOWN").into(),
                columns: columns.into_iter().take(256).collect(),
                foreign_keys: foreign_keys.into_iter().take(256).collect(),
                complete,
                sample: json!({"collected":false,"reason":"not_requested_or_owner_disabled"}),
                metadata: Some(metadata),
            };
            captured_bytes += serde_json::to_vec(&entry)
                .map_err(|_| remote_error())?
                .len();
            if captured_bytes > 8 * 1024 * 1024 {
                return Err(AppError::config("external schema capture exceeded 8 MiB while collecting; previous snapshot retained"));
            }
            tables.push(entry);
        }
    }
    if include_samples {
        if !t.samples_enabled {
            return Err(AppError::config(
                "row samples are disabled by owner configuration",
            ));
        }
        let mut budget = 65536;
        for table in &mut tables {
            table.sample = c.sample(t, table, &mut budget).await?;
        }
    }
    c.rollback().await?;
    let mut snapshot = Snapshot {
        capture_format: CAPTURE_FORMAT,
        id: uuid::Uuid::new_v4().to_string(),
        connection: alias.into(),
        project: t.project.clone(),
        dialect: match t.dialect {
            Dialect::Postgresql => "postgresql",
            Dialect::Mysql => "mysql",
        }
        .into(),
        schemas,
        captured_ms: chrono::Utc::now().timestamp_millis(),
        code_index_revision: revision,
        target_fingerprint: scope,
        tables,
        complete_within_visible_scope: diagnostics.is_empty(),
        diagnostics,
        inferred_relations: None,
    };
    snapshot.inferred_relations = Some(relations::infer(&snapshot));
    Ok(snapshot)
}
impl Store {
    pub async fn external_db_refresh(&self, q: &Refresh) -> Result<Value> {
        let raw = std::env::var("RAG_EXTERNAL_DATABASES").unwrap_or_else(|_| "{}".into());
        let all = targets(&raw)?;
        let target = all
            .get(&q.connection)
            .ok_or_else(|| AppError::config("unknown external database alias"))?;
        let url = std::env::var(&target.url_env)
            .map_err(|_| AppError::config("external database URL environment variable is unset"))?;
        let revision = self.code_snapshot()?.index_revision;
        let snapshot = tokio::time::timeout(
            Duration::from_secs(30),
            collect(&q.connection, target, &url, revision, q.include_samples),
        )
        .await
        .map_err(|_| {
            AppError::config("external database capture timed out; previous snapshot retained")
        })??;
        let body = serde_json::to_string(&snapshot).map_err(|_| remote_error())?;
        if body.len() > 8 * 1024 * 1024 {
            return Err(AppError::config(
                "external schema snapshot exceeds 8 MiB; reduce configured schemas",
            ));
        }
        self.lock()?.execute(
            "INSERT INTO external_db_snapshots (id,connection_alias,captured_ms,payload) VALUES (?,?,?,?)",
            duckdb::params![snapshot.id, snapshot.connection, snapshot.captured_ms, body],
        )?;
        Ok(
            json!({"snapshot_id":snapshot.id,"connection":snapshot.connection,"captured_ms":snapshot.captured_ms,"tables":snapshot.tables.len(),"complete_within_visible_scope":snapshot.complete_within_visible_scope,"diagnostics":snapshot.diagnostics,"source":"remote_metadata","capture_format":snapshot.capture_format,"samples_requested":q.include_samples,"readonly":true}),
        )
    }
    fn external_snapshot(&self, q: &Query) -> Result<Snapshot> {
        let conn = self.lock()?;
        let body: Option<String> = if let Some(id) = &q.snapshot_id {
            let mut s = conn.prepare(
                "SELECT payload FROM external_db_snapshots WHERE connection_alias=? AND id=?",
            )?;
            let mut rows = s.query(duckdb::params![q.connection, id])?;
            rows.next()?.map(|r| r.get(0)).transpose()?
        } else {
            let mut s=conn.prepare("SELECT payload FROM external_db_snapshots WHERE connection_alias=? ORDER BY seq DESC LIMIT 1")?;
            let mut rows = s.query(duckdb::params![q.connection])?;
            rows.next()?.map(|r| r.get(0)).transpose()?
        };
        let snapshot: Snapshot = serde_json::from_str(&body.ok_or_else(|| {
            AppError::NotFound("no captured snapshot for this alias; use db_refresh first".into())
        })?)
        .map_err(|_| AppError::config("stored external schema snapshot is invalid"))?;
        snapshot.validate_format()?;
        Ok(snapshot)
    }
    pub fn external_db_query(&self, operation: &str, q: &Query) -> Result<Value> {
        if q.connection.len() > 64 {
            return Err(AppError::config("invalid connection alias"));
        }
        if q.base_snapshot_id.is_some() && operation != "db_drift" {
            return Err(AppError::config(
                "base_snapshot_id is only supported by db_drift",
            ));
        }
        if matches!(operation, "db_drift" | "db_cross") && (q.schema.is_some() || q.table.is_some())
        {
            return Err(AppError::config("db_drift and db_cross compare the entire captured scope; schema/table filters are not supported"));
        }
        if q.relation_kind.is_some() && operation != "db_relations" {
            return Err(AppError::config(
                "relation_kind is only supported by db_relations",
            ));
        }
        let s = self.external_snapshot(q)?;
        if operation == "db_relations" {
            return relations::response(&s, q);
        }
        if operation == "db_drift" {
            return self.external_db_drift(&s, q);
        }
        if operation == "db_cross" {
            return self.external_db_cross(&s, q);
        }
        let selected: Vec<_> = s
            .tables
            .iter()
            .filter(|t| {
                q.schema.as_ref().is_none_or(|v| v == &t.schema)
                    && q.table.as_ref().is_none_or(|v| v == &t.name)
            })
            .collect();
        let limit = q.limit.unwrap_or(50).clamp(1, 200);
        let items:Vec<Value>=match operation {
            "db_schema"=>selected.iter().map(|t|json!({"schema":t.schema,"table":t.name,"kind":t.kind,"columns":t.columns.len(),"complete":t.complete})).collect(),
            "db_table"=>{if q.table.is_none(){return Err(AppError::config("db_table requires an exact table name"));} if selected.len()>1{return Err(AppError::config("table name is ambiguous; specify schema"));}selected.iter().map(|t|{let mut v=json!(t);v.as_object_mut().unwrap().remove("sample");v["column_traits"]=marks::summary(t,&s.dialect);
                v["inference_available"]=json!(s.inferred_relations.is_some());
                v["inferred_relations"]=json!(s.inferred_relations.as_ref().map(|a|a.relations.iter().filter(|r|r["schema"]==t.schema&&r["table"]==t.name).collect::<Vec<_>>()));
                v["inference_algorithm_version"]=json!(s.inferred_relations.as_ref().map(|a|a.algorithm_version));
                v["inference_analysis_truncated"]=json!(s.inferred_relations.as_ref().map(|a|a.truncated));v}).collect()},
            "db_sample"=>{if q.table.is_none(){return Err(AppError::config("db_sample requires an exact table name"));}if selected.len()>1{return Err(AppError::config("table name is ambiguous; specify schema"));}selected.iter().map(|t|json!({"schema":t.schema,"table":t.name,"sample":t.sample})).collect()},
            _=>return Err(AppError::config("unknown external database operation")),
        };
        Ok(
            json!({"snapshot_id":s.id,"connection":s.connection,"project":s.project,"dialect":s.dialect,"schemas":s.schemas,"captured_ms":s.captured_ms,"code_index_revision_at_capture":s.code_index_revision,"items":items.iter().take(limit).collect::<Vec<_>>(),"total":items.len(),"truncated":items.len()>limit,"diagnostics":s.diagnostics,"complete_within_visible_scope":s.complete_within_visible_scope,"scope":"metadata visible to configured database role in configured schemas","capture_format":s.capture_format,"not_collected":if s.capture_format<2{vec!["indexes","check_constraints","view_definitions","inferred_relations"]}else if s.capture_format<3{vec!["function_dependencies","inferred_relations","full_enum_domain_types","sequence_parameters"]}else if s.capture_format<4{vec!["function_dependencies","full_enum_domain_types","sequence_parameters"]}else{vec!["function_dependencies","composite_and_custom_type_bodies","sequence_parameters"]},"live_query":false}),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn owner_configuration_is_strict_and_does_not_echo_secrets() {
        for raw in [
            r#"{"x":{"dialect":"postgresql","url_env":"URL","project":"p","schemas":[]}}"#,
            r#"{"x":{"dialect":"mysql","url_env":"URL","project":"p","schemas":["app"],"password":"NEVER_ECHO"}}"#,
        ] {
            let e = targets(raw).err().unwrap().to_string();
            assert!(!e.contains("NEVER_ECHO"));
        }
        assert!(targets(r#"{"demo":{"dialect":"postgresql","url_env":"URL","project":"p","schemas":["public"]}}"#).is_ok());
        assert!(
            serde_json::from_value::<Refresh>(json!({"connection":"demo","url":"secret"})).is_err()
        );
    }
    #[test]
    fn snapshot_formats_are_explicit_and_future_payloads_are_rejected() {
        let payload = json!({"id":"old","connection":"fixture","project":"p","dialect":"postgresql","schemas":[],"captured_ms":0,"code_index_revision":null,"target_fingerprint":"h","tables":[],"complete_within_visible_scope":true,"diagnostics":[]});
        let old: Snapshot = serde_json::from_value(payload.clone()).unwrap();
        assert_eq!(old.capture_format, 1);
        old.validate_format().unwrap();
        let mut missing_analysis = payload.clone();
        missing_analysis["capture_format"] = json!(3);
        assert!(serde_json::from_value::<Snapshot>(missing_analysis)
            .unwrap()
            .validate_format()
            .is_err());
        let mut future = payload.clone();
        future["capture_format"] = json!(99);
        assert!(serde_json::from_value::<Snapshot>(future)
            .unwrap()
            .validate_format()
            .is_err());
        let mut incomplete = payload;
        incomplete["capture_format"] = json!(2);
        incomplete["tables"] = json!([{"schema":"public","name":"t","kind":"BASE TABLE","columns":[],"foreign_keys":[],"complete":true,"sample":null}]);
        assert!(serde_json::from_value::<Snapshot>(incomplete)
            .unwrap()
            .validate_format()
            .is_err());
    }
    #[test]
    fn scope_hash_ignores_schema_order_but_tracks_target() {
        let mut t = targets(
            r#"{"x":{"dialect":"mysql","url_env":"URL","project":"p","schemas":["a","b"]}}"#,
        )
        .unwrap()
        .remove("x")
        .unwrap();
        let a = fingerprint(&t, "host", 3306, "u", Some("db"));
        t.schemas.reverse();
        assert_eq!(a, fingerprint(&t, "host", 3306, "u", Some("db")));
        assert_ne!(a, fingerprint(&t, "other", 3306, "u", Some("db")));
    }
}

#[cfg(test)]
mod fixture_tests {
    use super::*;
    #[tokio::test]
    #[ignore = "requires owned disposable PostgreSQL/MySQL fixtures; scripts/smoke_external_db.py"]
    async fn native_metadata_capture_and_readonly_transactions() {
        for (dialect, var, schema) in [
            (Dialect::Postgresql, "RAG_TEST_EXTERNAL_PG_URL", "public"),
            (Dialect::Mysql, "RAG_TEST_EXTERNAL_MY_URL", "app"),
        ] {
            let url = std::env::var(var).expect("fixture URL required");
            let t = Target {
                dialect,
                url_env: var.into(),
                project: "fixture".into(),
                schemas: vec![schema.into()],
                tls: Tls::Disable,
                samples_enabled: false,
                sample_columns: BTreeMap::new(),
            };
            let snapshot = tokio::time::timeout(
                Duration::from_secs(30),
                collect("fixture", &t, &url, Some(17), false),
            )
            .await
            .unwrap()
            .unwrap();
            assert!(snapshot.complete_within_visible_scope, "{snapshot:?}");
            let inferred = snapshot.inferred_relations.as_ref().unwrap();
            assert_eq!(inferred.algorithm_version, 2);
            assert_eq!(inferred.relations.len(), 5, "{inferred:?}");
            assert!(
                inferred
                    .relations
                    .iter()
                    .all(|r| r["provenance"] == "inferred"
                        && r["confidence"].as_f64().unwrap() <= 0.4)
            );
            assert!(inferred
                .diagnostics
                .iter()
                .any(|d| d["column"] == "account_id"
                    && d["code"] == "TARGET_ID_NOT_PROVEN_INDIVIDUALLY_UNIQUE"));
            assert!(inferred.diagnostics.iter().any(
                |d| d["column"] == "report_user_id" && d["code"] == "IDENTIFIER_TYPE_MISMATCH"
            ));
            let event = snapshot.tables.iter().find(|t| t.name == "events").unwrap();
            assert!(
                event
                    .metadata
                    .as_ref()
                    .unwrap()
                    .types
                    .as_ref()
                    .unwrap()
                    .complete
            );
            assert!(inferred
                .relations
                .iter()
                .any(|r| r["mapping"]["column"] == "domain_user_id"));
            let defs = &event
                .metadata
                .as_ref()
                .unwrap()
                .types
                .as_ref()
                .unwrap()
                .definitions;
            match dialect {
                Dialect::Postgresql => {
                    let en = defs
                        .iter()
                        .find(|d| d["kind"] == "enum" && d["name"] == "order_status")
                        .unwrap();
                    assert_eq!(
                        en["enum_members"],
                        json!(["new", "in,progress", "quote'd", "slash\\path", "done"])
                    );
                    let domain = event
                        .columns
                        .iter()
                        .find(|c| c["name"] == "domain_user_id")
                        .unwrap();
                    assert_eq!(domain["nullable"], "NO");
                    assert!(domain["effective_default_sql"]
                        .as_str()
                        .unwrap()
                        .contains('9'));
                    assert_eq!(
                        domain["resolved_type"]["domain_chain"]
                            .as_array()
                            .unwrap()
                            .len(),
                        2
                    );
                    assert!(defs.iter().filter(|d| d["kind"] == "domain").all(|d| !d
                        ["constraints"]
                        .as_array()
                        .unwrap()
                        .is_empty()));
                    let array = event
                        .columns
                        .iter()
                        .find(|c| c["name"] == "optional_ids")
                        .unwrap();
                    assert_eq!(array["nullable"], "YES");
                    assert!(array["effective_default_sql"].is_null());
                    assert_eq!(array["resolved_type"]["kind"], "array");
                }
                Dialect::Mysql => {
                    let en = defs.iter().find(|d| d["name"] == "state").unwrap();
                    assert_eq!(
                        en["members"],
                        json!(["new", "in,progress", "quote'd", "slash\\path", "done"])
                    );
                    let set = defs.iter().find(|d| d["name"] == "permissions").unwrap();
                    assert_eq!(set["kind"], "set");
                    assert_eq!(
                        set["members"],
                        json!(["read", "write", "quote'd", "slash\\x"])
                    );
                }
            }
            let child = snapshot.tables.iter().find(|x| x.name == "child").unwrap();
            assert_eq!(child.foreign_keys.len(), 2, "{child:?}");
            assert_eq!(child.foreign_keys[0]["column"], "a");
            assert_eq!(child.foreign_keys[0]["target_column"], "a");
            assert_eq!(child.foreign_keys[1]["column"], "b");
            assert_eq!(child.foreign_keys[1]["target_column"], "b");
            assert!(child.columns.iter().any(|c| c["name"] == "computed"
                && !c["generation_expression"].as_str().unwrap_or("").is_empty()));
            assert!(snapshot.tables.iter().any(|x| x.name == "odd'name"));
            assert!(!serde_json::to_string(&snapshot)
                .unwrap()
                .contains("PRIVATE_ROW_MARKER"));
            let metadata = child.metadata.as_ref().unwrap();
            assert!(metadata.complete, "{metadata:?}");
            let pair = metadata
                .indexes
                .iter()
                .find(|i| i["name"] == "child_pair")
                .unwrap();
            assert_eq!(pair["keys"][0]["column"], "b");
            assert_eq!(pair["keys"][0]["direction"], "DESC");
            assert_eq!(pair["keys"][1]["column"], "a");
            let traits = marks::summary(
                child,
                match dialect {
                    Dialect::Postgresql => "postgresql",
                    Dialect::Mysql => "mysql",
                },
            );
            assert_eq!(traits["individually_unique_columns"], json!(["a\"b"]));
            assert_eq!(
                traits["unconditional_named_unique_keys"],
                json!([["a", "b"], ["a\"b"]])
            );
            let expression = metadata
                .indexes
                .iter()
                .find(|i| i["name"] == "child_expr")
                .unwrap();
            assert!(!expression["keys"][0]["expression"]
                .as_str()
                .unwrap()
                .is_empty());
            assert_eq!(
                expression["keys"][0]["expression_hash"]
                    .as_str()
                    .unwrap()
                    .len(),
                64
            );
            let check = metadata
                .constraints
                .iter()
                .find(|c| c["name"] == "child_nonnegative")
                .unwrap();
            assert_eq!(check["kind"], "CHECK");
            assert!(check["definition"].is_string());
            let primary = snapshot
                .tables
                .iter()
                .find(|t| t.name == "parent")
                .unwrap()
                .metadata
                .as_ref()
                .unwrap()
                .indexes
                .iter()
                .find(|i| i["primary"] == true)
                .unwrap();
            assert_eq!(primary["keys"][0]["column"], "a");
            assert_eq!(primary["keys"][1]["column"], "b");
            match dialect {
                Dialect::Postgresql => {
                    let nulls = metadata
                        .indexes
                        .iter()
                        .find(|i| i["name"] == "child_nulls_equal")
                        .unwrap();
                    assert_eq!(nulls["nulls_not_distinct"], true);
                    let deferred = metadata
                        .constraints
                        .iter()
                        .find(|c| c["name"] == "child_unique")
                        .unwrap();
                    assert_eq!(deferred["deferrable"], true);
                    assert_eq!(deferred["initially_deferred"], true);
                    let identity = traits["columns"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|c| c["column"] == "id")
                        .unwrap();
                    assert_eq!(identity["identity"], true);
                    assert_eq!(identity["identity_kind"], "BY DEFAULT");
                    assert_eq!(
                        child
                            .columns
                            .iter()
                            .find(|c| c["name"] == "computed")
                            .unwrap()["generation_kind"],
                        "STORED"
                    );
                    assert_eq!(identity["generated_expression"], false);
                    assert_eq!(check["validated"], false);
                    assert_eq!(check["enforced"], true);
                    assert_eq!(expression["keys"][1]["included"], true);
                    assert!(expression["predicate"]
                        .as_str()
                        .unwrap()
                        .contains("IS NULL"));
                    assert!(metadata
                        .constraints
                        .iter()
                        .any(|c| c["kind"] == "EXCLUSION"));
                    let mv = snapshot
                        .tables
                        .iter()
                        .find(|t| t.name == "child_mv")
                        .unwrap();
                    assert_eq!(mv.kind, "MATERIALIZED VIEW");
                    assert_eq!(mv.columns[0]["name"], "b");
                    assert!(
                        mv.metadata
                            .as_ref()
                            .unwrap()
                            .view
                            .as_ref()
                            .unwrap()
                            .complete
                    );
                }
                Dialect::Mysql => {
                    assert_eq!(check["enforced"], false);
                    assert!(check["validated"].is_null());
                    let prefix = metadata
                        .indexes
                        .iter()
                        .find(|i| i["name"] == "child_note_prefix")
                        .unwrap();
                    assert_eq!(prefix["keys"][0]["prefix_length"], "12");
                    assert_eq!(prefix["visible"], false);
                    let identity = traits["columns"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|c| c["column"] == "id")
                        .unwrap();
                    assert_eq!(identity["auto_increment"], true);
                    assert_eq!(identity["generated_expression"], false);
                    let default = traits["columns"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .find(|c| c["column"] == "created_at")
                        .unwrap();
                    assert_eq!(default["generated_expression"], false);
                    assert!(child
                        .columns
                        .iter()
                        .find(|c| c["name"] == "created_at")
                        .unwrap()["generation_metadata"]
                        .as_str()
                        .unwrap()
                        .contains("DEFAULT_GENERATED"));
                    let admin_url = std::env::var(format!("{var}_ADMIN")).unwrap();
                    let options = sqlx::mysql::MySqlConnectOptions::from_str(&admin_url)
                        .unwrap()
                        .ssl_mode(sqlx::mysql::MySqlSslMode::Disabled)
                        .disable_statement_logging();
                    let mut admin = sqlx::MySqlConnection::connect_with(&options).await.unwrap();
                    let old_mode: String =
                        sqlx::query_scalar("SELECT CAST(@@GLOBAL.sql_mode AS CHAR)")
                            .fetch_one(&mut admin)
                            .await
                            .unwrap();
                    admin
                        .execute("SET GLOBAL sql_mode='NO_BACKSLASH_ESCAPES'")
                        .await
                        .unwrap();
                    admin
                        .execute("SET SESSION sql_mode='NO_BACKSLASH_ESCAPES'")
                        .await
                        .unwrap();
                    admin.execute("CREATE TABLE app.type_literal_probe(value ENUM('slash\\q','quote''d','comma,value'))").await.unwrap();
                    let no_backslash = collect("mode_probe", &t, &url, Some(17), false)
                        .await
                        .unwrap();
                    admin
                        .execute("DROP TABLE app.type_literal_probe")
                        .await
                        .unwrap();
                    sqlx::query("SET GLOBAL sql_mode=?")
                        .bind(old_mode)
                        .execute(&mut admin)
                        .await
                        .unwrap();
                    let probe = &no_backslash
                        .tables
                        .iter()
                        .find(|t| t.name == "type_literal_probe")
                        .unwrap()
                        .metadata
                        .as_ref()
                        .unwrap()
                        .types
                        .as_ref()
                        .unwrap()
                        .definitions;
                    assert_eq!(
                        probe[0]["members"],
                        json!(["slash\\q", "quote'd", "comma,value"])
                    );
                    let other = &no_backslash
                        .tables
                        .iter()
                        .find(|t| t.name == "events")
                        .unwrap()
                        .metadata
                        .as_ref()
                        .unwrap()
                        .types
                        .as_ref()
                        .unwrap()
                        .definitions;
                    assert_eq!(
                        other.iter().find(|d| d["name"] == "state").unwrap()["members"],
                        defs.iter().find(|d| d["name"] == "state").unwrap()["members"]
                    );
                    assert_eq!(
                        other.iter().find(|d| d["name"] == "permissions").unwrap()["members"],
                        defs.iter().find(|d| d["name"] == "permissions").unwrap()["members"]
                    );
                    assert_eq!(
                        other, defs,
                        "reader SQL mode must not alter normalized declared type metadata"
                    );
                    let limited_url = std::env::var(format!("{var}_LIMITED")).unwrap();
                    let limited = collect("limited", &t, &limited_url, Some(17), false)
                        .await
                        .unwrap();
                    assert!(!limited.complete_within_visible_scope);
                    let hidden_view = limited
                        .tables
                        .iter()
                        .find(|t| t.name == "child_view")
                        .unwrap()
                        .metadata
                        .as_ref()
                        .unwrap()
                        .view
                        .as_ref()
                        .unwrap();
                    assert!(!hidden_view.complete);
                    assert!(hidden_view.definition.is_none());
                    assert!(hidden_view.definition_hash.is_none());
                    assert!(limited
                        .diagnostics
                        .iter()
                        .any(|d| d.starts_with("VIEW_DEFINITION_UNAVAILABLE")));
                    assert!(limited
                        .tables
                        .iter()
                        .any(|t| t.name == "child" && t.metadata.as_ref().unwrap().complete));
                }
            }
            for (view, target) in [("child_view", "child"), ("nested_view", "child_view")] {
                let v = snapshot
                    .tables
                    .iter()
                    .find(|t| t.name == view)
                    .unwrap()
                    .metadata
                    .as_ref()
                    .unwrap()
                    .view
                    .as_ref()
                    .unwrap();
                assert!(v.complete, "{v:?}");
                assert!(v.definition.as_ref().unwrap().contains(target));
                assert!(v.dependencies.iter().any(|d| d["target_table"] == target));
            }
            let mut verified = t.clone();
            verified.tls = Tls::VerifyFull;
            assert!(
                connect(&verified, &url).await.is_err(),
                "fixture has no trusted hostname certificate; TLS must not silently downgrade"
            );
            let mut sampling = t.clone();
            sampling.samples_enabled = true;
            sampling.sample_columns.insert(
                schema.into(),
                BTreeMap::from([("child".into(), vec!["a".into(), "b".into(), "email".into()])]),
            );
            sampling.sample_columns.get_mut(schema).unwrap().insert(
                "events".into(),
                vec![
                    "state".into(),
                    "states".into(),
                    "permissions".into(),
                    "domain_user_id".into(),
                ],
            );
            let sampled = collect("fixture", &sampling, &url, Some(17), true)
                .await
                .unwrap();
            let sample = &sampled
                .tables
                .iter()
                .find(|t| t.name == "child")
                .unwrap()
                .sample;
            let typed_sample = &sampled
                .tables
                .iter()
                .find(|t| t.name == "events")
                .unwrap()
                .sample;
            assert_eq!(typed_sample["collected"], true);
            assert_eq!(typed_sample["rows"][0]["state"]["value"], "in,progress");
            if matches!(dialect, Dialect::Postgresql) {
                assert_eq!(typed_sample["rows"][0]["domain_user_id"]["value"], "9");
                assert!(typed_sample["rows"][0]["states"]["value"]
                    .as_str()
                    .unwrap()
                    .contains("quote'd"));
            } else {
                assert_eq!(
                    typed_sample["rows"][0]["permissions"]["value"],
                    "read,write"
                );
            }
            assert_eq!(sample["collected"], true);
            assert_eq!(sample["rows"][0]["a"], Value::Null);
            assert!(sample["columns"]
                .as_array()
                .unwrap()
                .iter()
                .all(|c| c != "email"));
            assert!(!serde_json::to_string(&sampled)
                .unwrap()
                .contains("PRIVATE_ROW_MARKER"));
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("capture.duckdb");
            let store = Store::open(&path).unwrap();
            store
                .lock()
                .unwrap()
                .execute(
                    "INSERT INTO external_db_snapshots (id,connection_alias,captured_ms,payload) VALUES (?,?,?,?)",
                    duckdb::params![
                        snapshot.id,
                        "fixture",
                        snapshot.captured_ms,
                        serde_json::to_string(&snapshot).unwrap()
                    ],
                )
                .unwrap();
            let q = Query {
                connection: "fixture".into(),
                table: Some("child".into()),
                ..Default::default()
            };
            assert_eq!(
                store.external_db_query("db_table", &q).unwrap()["items"][0]["foreign_keys"]
                    .as_array()
                    .unwrap()
                    .len(),
                2
            );
            assert!(store
                .external_db_query(
                    "db_schema",
                    &Query {
                        connection: "fixture' OR 1=1 --".into(),
                        ..Default::default()
                    }
                )
                .is_err());
            assert_eq!(
                store
                    .external_db_query("db_drift", &q)
                    .unwrap_err()
                    .to_string()
                    .contains("filters are not supported"),
                true
            );
            let drift_q = Query {
                connection: "fixture".into(),
                ..Default::default()
            };
            assert_eq!(
                store.external_db_query("db_drift", &drift_q).unwrap()["compared"],
                false
            );
            let mut next = snapshot.clone();
            next.tables[0].sample = json!({"collected":true,"rows":[{"status":"changed data"}]});
            next.id = uuid::Uuid::new_v4().to_string();
            // Identical timestamps must still select actual insertion order.
            store.lock().unwrap().execute("INSERT INTO external_db_snapshots (id,connection_alias,captured_ms,payload) VALUES (?,?,?,?)",duckdb::params![next.id,"fixture",next.captured_ms,serde_json::to_string(&next).unwrap()]).unwrap();
            let unchanged = store.external_db_query("db_drift", &drift_q).unwrap();
            assert_eq!(unchanged["snapshot_id"], next.id);
            assert_eq!(unchanged["total"], 0);
            next.id = uuid::Uuid::new_v4().to_string();
            next.tables
                .iter_mut()
                .find(|t| t.name == "child")
                .unwrap()
                .columns[0]["nullable"] = json!("NO");
            store.lock().unwrap().execute("INSERT INTO external_db_snapshots (id,connection_alias,captured_ms,payload) VALUES (?,?,?,?)",duckdb::params![next.id,"fixture",next.captured_ms,serde_json::to_string(&next).unwrap()]).unwrap();
            assert_eq!(
                store.external_db_query("db_drift", &drift_q).unwrap()["changes"][0]["kind"],
                "table_changed"
            );
            for field in ["indexes", "constraints", "view"] {
                let mut changed = snapshot.clone();
                changed.id = uuid::Uuid::new_v4().to_string();
                let table = changed
                    .tables
                    .iter_mut()
                    .find(|t| {
                        t.name
                            == if field == "view" {
                                "child_view"
                            } else {
                                "child"
                            }
                    })
                    .unwrap();
                let metadata = table.metadata.as_mut().unwrap();
                match field {
                    "indexes" => {
                        metadata.indexes[0]["unique"] =
                            json!(!metadata.indexes[0]["unique"].as_bool().unwrap())
                    }
                    "constraints" => {
                        metadata.constraints[0]["definition"] = json!("changed constraint")
                    }
                    _ => {
                        metadata.view.as_mut().unwrap().definition_hash =
                            Some("changed definition".into())
                    }
                };
                store.lock().unwrap().execute("INSERT INTO external_db_snapshots (id,connection_alias,captured_ms,payload) VALUES (?,?,?,?)",duckdb::params![changed.id,"fixture",changed.captured_ms,serde_json::to_string(&changed).unwrap()]).unwrap();
                let diff = store
                    .external_db_query(
                        "db_drift",
                        &Query {
                            base_snapshot_id: Some(snapshot.id.clone()),
                            ..Query {
                                connection: "fixture".into(),
                                ..Default::default()
                            }
                        },
                    )
                    .unwrap();
                assert_eq!(diff["total"], 1, "{field}:{diff}");
            }
            // A legacy payload is readable but never compared as though missing metadata were absent objects.
            let mut legacy = serde_json::to_value(&snapshot).unwrap();
            legacy.as_object_mut().unwrap().remove("capture_format");
            legacy.as_object_mut().unwrap().remove("inferred_relations");
            legacy["id"] = json!(uuid::Uuid::new_v4().to_string());
            for t in legacy["tables"].as_array_mut().unwrap() {
                t.as_object_mut().unwrap().remove("metadata");
            }
            let legacy_id = legacy["id"].as_str().unwrap().to_string();
            store.lock().unwrap().execute("INSERT INTO external_db_snapshots (id,connection_alias,captured_ms,payload) VALUES (?,?,?,?)",duckdb::params![legacy_id,"fixture",snapshot.captured_ms,legacy.to_string()]).unwrap();
            assert_eq!(
                store
                    .external_db_query(
                        "db_table",
                        &Query {
                            snapshot_id: Some(legacy_id.clone()),
                            connection: "fixture".into(),
                            table: Some("child".into()),
                            ..Default::default()
                        }
                    )
                    .unwrap()["capture_format"],
                1
            );
            let legacy_diff = store
                .external_db_query(
                    "db_drift",
                    &Query {
                        connection: "fixture".into(),
                        snapshot_id: Some(snapshot.id.clone()),
                        base_snapshot_id: Some(legacy_id),
                        ..Default::default()
                    },
                )
                .unwrap();
            assert_eq!(legacy_diff["reason"], "capture_format_changed");
            let sources = root.path().join("sources");
            std::fs::create_dir(&sources).unwrap();
            std::fs::write(sources.join("001.sql"),r#"CREATE TABLE child (a INT, b INT, missing TEXT, "a""b" INT); SELECT * FROM child; INSERT INTO absent_table(a) VALUES(1);"#).unwrap();
            let config = crate::Config {
                ingest_roots: vec![sources.clone()],
                embedding_dims: 16,
                ..crate::Config::for_tests()
            };
            let embedder: std::sync::Arc<dyn crate::embeddings::EmbeddingProvider> =
                std::sync::Arc::new(crate::embeddings::MockEmbedder::new(16));
            let mut command = crate::source_sync::SourceSyncCommand::new(sources);
            command.wing = Some("fixture".into());
            let service = crate::source_sync::SourceSyncService::new(&store, &embedder, &config);
            assert!(service.sync(command).await.unwrap().errors.is_empty());
            drop(service);
            let cross = store.external_db_query("db_cross", &drift_q).unwrap();
            assert!(cross["uses"]
                .as_array()
                .unwrap()
                .iter()
                .any(|u| u["table"] == "absent_table"
                    && u["resolution"] == "not_observed_in_visible_live_schema"));
            assert!(
                cross["migrations"][0]["migration_columns_not_observed_live"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|c| c == "missing")
            );
            assert_eq!(
                cross["migrations"][0]["migration_columns_not_observed_live"],
                json!(["missing"])
            );
            assert!(cross["live_tables_without_observed_static_sql_use"]
                .as_array()
                .unwrap()
                .iter()
                .all(|t| t["deletion_safe"] == false));
            next.id = uuid::Uuid::new_v4().to_string();
            next.target_fingerprint = "different-target".into();
            store.lock().unwrap().execute("INSERT INTO external_db_snapshots (id,connection_alias,captured_ms,payload) VALUES (?,?,?,?)",duckdb::params![next.id,"fixture",next.captured_ms,serde_json::to_string(&next).unwrap()]).unwrap();
            assert_eq!(
                store
                    .external_db_query(
                        "db_drift",
                        &Query {
                            connection: "fixture".into(),
                            base_snapshot_id: Some(snapshot.id.clone()),
                            ..Default::default()
                        }
                    )
                    .unwrap()["reason"],
                "different_database_target_or_visible_scope"
            );
            drop(store);
            let store = Store::open(&path).unwrap();
            assert_eq!(
                store.external_db_query("db_relations", &q).unwrap()["total"],
                2
            );
            let admin_url = std::env::var(format!("{var}_ADMIN")).unwrap();
            let (mut over_budget, _) = connect(&t, &admin_url).await.unwrap();
            let error = over_budget
                .rows(
                    "SELECT repeat('x',8388609)::text",
                    "SELECT CAST(REPEAT('x',8388609) AS CHAR)",
                    &[],
                    &["oversize"],
                )
                .await
                .unwrap_err();
            assert!(error.to_string().contains("8 MiB"));
            drop(over_budget);
            let (mut c, _) = connect(&t, &admin_url).await.unwrap();
            match &mut c {
                Remote::Pg(c) => {
                    let e = c
                        .execute("INSERT INTO public.parent VALUES (9,9)")
                        .await
                        .unwrap_err();
                    assert_eq!(
                        e.as_database_error().unwrap().code().as_deref(),
                        Some("25006")
                    );
                }
                Remote::My(c) => {
                    let e = c
                        .execute("INSERT INTO app.parent VALUES (9,9)")
                        .await
                        .unwrap_err();
                    assert_eq!(
                        e.as_database_error().unwrap().code().as_deref(),
                        Some("25006")
                    );
                }
            }
            c.rollback().await.unwrap();
        }
    }
}
