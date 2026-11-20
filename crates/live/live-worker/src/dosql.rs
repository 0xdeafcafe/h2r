//! The `LiveHub` Durable Object's own SQLite storage (`ctx.storage.sql`)
//! as the store's [`Sql`]. It runs in the object's process and
//! synchronously, so a query costs no network hop (D1's does).
//!
//! A batch runs inside `storage.transactionSync`, so it commits whole or
//! not at all: an error makes the callback throw, which rolls it back.
//!
//! The storage object is JavaScript and not `Send`. A Worker has one
//! thread, so it is wrapped ([`SendWrapper`]) to meet the store's trait.
//! No call here awaits.

use async_trait::async_trait;
use js_sys::{Function, Reflect};
use live_server::store::StoreError;
use send_wrapper::SendWrapper;
use serde_json::Value;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use worker::{SqlStorage, SqlStorageValue};

use crate::sqlstore::{Param, Row, Sql, Statement, script_statements};

pub struct DoSql {
    sql: SendWrapper<SqlStorage>,
    /// `ctx.storage`, for `transactionSync` (which workers-rs does not
    /// bind).
    storage: SendWrapper<JsValue>,
}

impl DoSql {
    /// `state` is the object's `ctx` (`State::_inner`).
    pub fn new(sql: SqlStorage, state: &JsValue) -> worker::Result<Self> {
        let storage = Reflect::get(state, &"storage".into())?;
        Ok(DoSql {
            sql: SendWrapper::new(sql),
            storage: SendWrapper::new(storage),
        })
    }
}

fn backend(e: impl std::fmt::Display) -> StoreError {
    StoreError::Backend(e.to_string())
}

fn value(p: &Param) -> SqlStorageValue {
    match p {
        Param::Null => SqlStorageValue::Null,
        Param::Int(n) => SqlStorageValue::Integer(*n),
        Param::Text(s) => SqlStorageValue::String(s.clone()),
        Param::Bytes(b) => SqlStorageValue::Blob(b.clone()),
    }
}

fn json(v: SqlStorageValue) -> Value {
    match v {
        SqlStorageValue::Null => Value::Null,
        SqlStorageValue::Boolean(b) => Value::Bool(b),
        SqlStorageValue::Integer(n) => Value::from(n),
        SqlStorageValue::Float(f) => Value::from(f),
        SqlStorageValue::String(s) => Value::String(s),
        SqlStorageValue::Blob(b) => Value::from(b),
    }
}

/// Runs `sql` and reads every row.
fn run(storage: &SqlStorage, sql: &str, params: &[Param]) -> worker::Result<Vec<Row>> {
    let cursor = storage.exec(sql, params.iter().map(value).collect::<Vec<_>>())?;
    let columns = cursor.column_names();
    cursor
        .raw()
        .map(|row| {
            Ok(columns
                .iter()
                .cloned()
                .zip(row?.into_iter().map(json))
                .collect())
        })
        .collect()
}

impl DoSql {
    /// `body` inside `storage.transactionSync`: committed if it returns
    /// `Ok`, rolled back (the callback throws) if not.
    fn transaction<T: 'static>(
        &self,
        body: impl FnOnce(&SqlStorage) -> worker::Result<T> + 'static,
    ) -> Result<T, StoreError> {
        let out = std::rc::Rc::new(std::cell::RefCell::new(None));
        let sql: SqlStorage = (*self.sql).clone();
        let slot = out.clone();
        let callback = Closure::once(move || -> Result<(), JsValue> {
            let result = body(&sql).map_err(|e| JsValue::from_str(&e.to_string()))?;
            *slot.borrow_mut() = Some(result);
            Ok(())
        });
        let transaction_sync: Function = Reflect::get(&self.storage, &"transactionSync".into())
            .and_then(|f| f.dyn_into().map_err(|_| "no transactionSync".into()))
            .map_err(|e| backend(format!("{e:?}")))?;
        transaction_sync
            .call1(&self.storage, callback.as_ref().unchecked_ref())
            .map_err(|e| backend(e.as_string().unwrap_or_else(|| format!("{e:?}"))))?;
        out.borrow_mut()
            .take()
            .ok_or_else(|| backend("the transaction did not run"))
    }
}

#[async_trait]
impl Sql for DoSql {
    async fn all(&self, sql: &str, params: Vec<Param>) -> Result<Vec<Row>, StoreError> {
        run(&self.sql, sql, &params).map_err(backend)
    }

    async fn batch(&self, statements: Vec<Statement>) -> Result<Vec<Vec<Row>>, StoreError> {
        self.transaction(move |sql| {
            statements
                .iter()
                .map(|(s, params)| run(sql, s, params))
                .collect()
        })
    }

    async fn script(&self, sql: &str) -> Result<(), StoreError> {
        let statements = script_statements(sql);
        self.transaction(move |storage| {
            for s in &statements {
                run(storage, s, &[])?;
            }
            Ok(())
        })
    }
}
