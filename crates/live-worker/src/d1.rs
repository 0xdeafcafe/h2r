//! The D1 binding as the store's [`Sql`]. D1 is JavaScript and not `Send`;
//! a Worker has one thread, so the binding and its futures are wrapped
//! ([`SendWrapper`]) to meet the store's trait.

use async_trait::async_trait;
use live_server::store::StoreError;
use send_wrapper::SendWrapper;
use wasm_bindgen::JsValue;
use worker::{D1Database, D1PreparedStatement};

use crate::d1store::{Param, Row, Sql, Statement, script_statements};

pub struct D1(SendWrapper<D1Database>);

impl D1 {
    pub fn new(db: D1Database) -> Self {
        D1(SendWrapper::new(db))
    }
}

fn backend(e: worker::Error) -> StoreError {
    StoreError::Backend(e.to_string())
}

fn js(p: &Param) -> JsValue {
    match p {
        Param::Null => JsValue::NULL,
        // D1 takes numbers as JavaScript's (53 bits): positions and
        // versions fit.
        Param::Int(n) => JsValue::from_f64(*n as f64),
        Param::Text(s) => JsValue::from_str(s),
        Param::Bytes(b) => js_sys::Uint8Array::from(b.as_slice()).into(),
    }
}

fn prepare(db: &D1Database, sql: &str, params: &[Param]) -> worker::Result<D1PreparedStatement> {
    let values: Vec<JsValue> = params.iter().map(js).collect();
    db.prepare(sql).bind(&values)
}

#[async_trait]
impl Sql for D1 {
    async fn all(&self, sql: &str, params: Vec<Param>) -> Result<Vec<Row>, StoreError> {
        let db = &self.0;
        SendWrapper::new(async move {
            let result = prepare(db, sql, &params)?.all().await?;
            result.results::<Row>()
        })
        .await
        .map_err(backend)
    }

    async fn batch(&self, statements: Vec<Statement>) -> Result<Vec<Vec<Row>>, StoreError> {
        let db = &self.0;
        SendWrapper::new(async move {
            let prepared = statements
                .iter()
                .map(|(sql, params)| prepare(db, sql, params))
                .collect::<worker::Result<Vec<_>>>()?;
            let results = db.batch(prepared).await?;
            results
                .iter()
                .map(|r| r.results::<Row>())
                .collect::<worker::Result<Vec<_>>>()
        })
        .await
        .map_err(backend)
    }

    async fn script(&self, sql: &str) -> Result<(), StoreError> {
        let db = &self.0;
        let statements = script_statements(sql);
        SendWrapper::new(async move {
            let prepared = statements
                .iter()
                .map(|s| Ok(db.prepare(s.as_str())))
                .collect::<worker::Result<Vec<_>>>()?;
            db.batch(prepared).await?;
            Ok(())
        })
        .await
        .map_err(backend)
    }
}
