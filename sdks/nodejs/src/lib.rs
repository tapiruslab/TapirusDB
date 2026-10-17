#![deny(clippy::all)]
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{
    ErrorStrategy, ThreadSafeCallContext, ThreadsafeFunction, ThreadsafeFunctionCallMode,
};
use napi_derive::napi;
use std::collections::HashMap;
use tapirus::{Connection, Value};

fn rows_to_js(env: Env, rows: Vec<tapirus::Row>) -> Result<napi::JsObject> {
    let mut array = env.create_array_with_length(rows.len())?;
    for (index, row) in rows.into_iter().enumerate() {
        let mut object = env.create_object()?;
        for (name, value) in row.columns().iter().zip(row.values()) {
            match value {
                Value::Null => object.set(name, env.get_null()?)?,
                Value::Integer(v) if v.unsigned_abs() > 9_007_199_254_740_991 => {
                    object.set(name, env.create_bigint_from_i64(*v)?)?
                }
                Value::Integer(v) => object.set(name, env.create_int64(*v)?)?,
                Value::Real(v) => object.set(name, env.create_double(*v)?)?,
                Value::Text(v) => object.set(name, env.create_string(v)?)?,
                Value::Blob(v) => {
                    object.set(name, env.create_buffer_with_data(v.clone())?.into_raw())?
                }
                Value::Vector(v) => {
                    let mut vector = env.create_array_with_length(v.len())?;
                    for (i, value) in v.iter().enumerate() {
                        vector.set_element(i as u32, env.create_double(f64::from(*value))?)?;
                    }
                    object.set(name, vector)?;
                }
            }
        }
        array.set_element(index as u32, object)?;
    }
    Ok(array)
}

#[napi]
pub struct TapirusClient {
    inner: Option<Connection>,
    subscriptions: HashMap<
        u32,
        (
            u64,
            ThreadsafeFunction<String, ErrorStrategy::CalleeHandled>,
        ),
    >,
}
#[napi(object)]
pub struct TapirusOptions {
    pub db_path: String,
    pub passphrase: Option<String>,
    pub memory_config_json: Option<String>,
}

impl TapirusOptions {
    fn configuration(&self) -> Result<tapirus::Config> {
        Ok(tapirus::Config {
            passphrase: self.passphrase.clone(),
            memory: self
                .memory_config_json
                .as_deref()
                .map(serde_json::from_str)
                .transpose()
                .map_err(|e| Error::from_reason(format!("Invalid memory configuration: {e}")))?
                .unwrap_or_default(),
            ..Default::default()
        })
    }
}

fn parameters(json: Option<String>) -> Result<Vec<Value>> {
    let values: Vec<serde_json::Value> = serde_json::from_str(json.as_deref().unwrap_or("[]"))
        .map_err(|e| Error::from_reason(e.to_string()))?;
    values
        .iter()
        .map(Value::from_json_parameter)
        .collect::<tapirus::Result<Vec<_>>>()
        .map_err(|e| Error::from_reason(e.to_string()))
}
impl TapirusClient {
    fn connection(&self) -> Result<&Connection> {
        self.inner
            .as_ref()
            .ok_or_else(|| Error::from_reason("Database connection is closed"))
    }
}
#[napi]
impl TapirusClient {
    #[napi(constructor)]
    pub fn new(options: TapirusOptions) -> Result<Self> {
        let config = options.configuration()?;
        let conn = if options.db_path == ":memory:" {
            Connection::open_in_memory_with_config(config)
        } else {
            Connection::open_with_config(&options.db_path, config)
        }
        .map_err(|e| Error::from_reason(e.to_string()))?;
        Ok(Self {
            inner: Some(conn),
            subscriptions: HashMap::new(),
        })
    }
    #[napi]
    pub fn execute(&self, sql: String, params_json: Option<String>) -> Result<u32> {
        let params = parameters(params_json)?;
        let count = self
            .connection()?
            .execute_with_params(&sql, &params)
            .map_err(|e| Error::from_reason(e.to_string()))?;
        u32::try_from(count).map_err(|_| Error::from_reason("Affected row count exceeds u32 range"))
    }
    #[napi]
    pub fn query(
        &self,
        env: Env,
        sql: String,
        params_json: Option<String>,
    ) -> Result<napi::JsObject> {
        let params = parameters(params_json)?;
        let rows = self
            .connection()?
            .query_with_params(&sql, &params)
            .map_err(|e| Error::from_reason(e.to_string()))?;
        rows_to_js(env, rows)
    }
    #[napi]
    pub fn checkpoint(&self) -> Result<u32> {
        let count = self
            .connection()?
            .checkpoint()
            .map_err(|e| Error::from_reason(e.to_string()))?;
        u32::try_from(count).map_err(|_| Error::from_reason("Page count exceeds u32 range"))
    }
    #[napi]
    pub fn close(&mut self) -> Result<()> {
        if let Some(conn) = &self.inner {
            for (_, (subscription_id, _)) in self.subscriptions.drain() {
                conn.unsubscribe(subscription_id);
            }
            conn.close()
                .map_err(|e| Error::from_reason(e.to_string()))?;
        }
        self.inner.take();
        Ok(())
    }
    #[napi]
    pub fn subscribe(&mut self, table: String, callback: JsFunction) -> Result<u32> {
        let tsfn: ThreadsafeFunction<String, ErrorStrategy::CalleeHandled> = callback
            .create_threadsafe_function(0, |ctx: ThreadSafeCallContext<String>| {
                Ok(vec![ctx.env.create_string(&ctx.value)?])
            })?;
        let listener = tsfn.clone();
        let subscription_id = self.connection()?.subscribe(&table, move |event| {
            if let Ok(json) = serde_json::to_string(event) {
                let _ = listener.call(Ok(json), ThreadsafeFunctionCallMode::NonBlocking);
            }
        });
        let public_id = u32::try_from(subscription_id)
            .map_err(|_| Error::from_reason("Subscription identifier exceeds u32 range"))?;
        self.subscriptions
            .insert(public_id, (subscription_id, tsfn));
        Ok(public_id)
    }
    #[napi]
    pub fn unsubscribe(&mut self, subscription_id: u32) -> Result<bool> {
        let Some((native_id, _callback)) = self.subscriptions.remove(&subscription_id) else {
            return Ok(false);
        };
        self.connection()?.unsubscribe(native_id);
        Ok(true)
    }
    #[napi]
    pub fn tap_classify_json(&self, text: String, candidates: Vec<String>) -> Result<String> {
        let slices: Vec<_> = candidates.iter().map(String::as_str).collect();
        let result = self
            .connection()?
            .tap()
            .classify(&text, &slices)
            .map_err(|e| Error::from_reason(e.to_string()))?;
        serde_json::to_string(&result).map_err(|e| Error::from_reason(e.to_string()))
    }
    #[napi]
    pub fn tap_verify(
        &self,
        premise: String,
        hypothesis: String,
        threshold: Option<f64>,
    ) -> Result<bool> {
        let threshold = threshold.unwrap_or(0.5);
        if !threshold.is_finite() || !(0.0..=1.0).contains(&threshold) {
            return Err(Error::from_reason("Threshold must be between 0 and 1"));
        }
        let result = self
            .connection()?
            .tap()
            .verify(&premise, &hypothesis)
            .map_err(|e| Error::from_reason(e.to_string()))?;
        Ok(f64::from(result.confidence) >= threshold)
    }
    #[napi]
    pub fn tap_score(&self, text: String, criteria: String) -> Result<f64> {
        self.connection()?
            .tap()
            .score(&text, &criteria)
            .map(|r| f64::from(r.score))
            .map_err(|e| Error::from_reason(e.to_string()))
    }
    #[napi]
    pub fn tap_route(&self, state: String, routes: Vec<String>) -> Result<String> {
        let slices: Vec<_> = routes.iter().map(String::as_str).collect();
        self.connection()?
            .tap()
            .route(&state, &slices)
            .map(|r| r.selected_route)
            .map_err(|e| Error::from_reason(e.to_string()))
    }
    #[napi]
    pub fn remember(
        &self,
        text: String,
        importance: Option<f64>,
        tags: Option<Vec<String>>,
    ) -> Result<f64> {
        let tags = tags.unwrap_or_default();
        let slices: Vec<_> = tags.iter().map(String::as_str).collect();
        self.connection()?
            .memory_remember_text(&text, importance.unwrap_or(0.5) as f32, &slices)
            .map(|id| id as f64)
            .map_err(|e| Error::from_reason(e.to_string()))
    }
    #[napi]
    pub fn store_memory_json(&self, document_json: String) -> Result<f64> {
        let document = serde_json::from_str::<tapirus::MemoryDocument>(&document_json)
            .map_err(|error| Error::from_reason(error.to_string()))?;
        self.connection()?
            .store_memory(&document)
            .map(|id| id as f64)
            .map_err(|error| Error::from_reason(error.to_string()))
    }
    #[napi]
    pub fn memory_request_json(&self, operation: String, payload: String) -> Result<String> {
        self.connection()?
            .memory_request_json(&operation, &payload)
            .map_err(|error| Error::from_reason(error.to_string()))
    }
    #[napi]
    pub fn search_memories_json(&self, search_json: String) -> Result<String> {
        let search = serde_json::from_str::<tapirus::MemorySearch>(&search_json)
            .map_err(|error| Error::from_reason(error.to_string()))?;
        let results = self
            .connection()?
            .search_memories(&search)
            .map_err(|error| Error::from_reason(error.to_string()))?;
        serde_json::to_string(&results).map_err(|error| Error::from_reason(error.to_string()))
    }
    #[napi]
    pub fn forget_memory(&self, id: String) -> Result<bool> {
        let id = id
            .parse::<u64>()
            .map_err(|error| Error::from_reason(error.to_string()))?;
        self.connection()?
            .memory_forget_checked(id)
            .map_err(|error| Error::from_reason(error.to_string()))
    }
    #[napi]
    pub fn recall_json(&self, query: String, limit: Option<u32>) -> Result<String> {
        let result = self
            .connection()?
            .memory_recall_text(&query, limit.unwrap_or(5) as usize);
        serde_json::to_string(&result).map_err(|e| Error::from_reason(e.to_string()))
    }
    #[napi]
    pub fn recall_prompt(&self, query: String, limit: Option<u32>) -> Result<String> {
        Ok(self
            .connection()?
            .recall_prompt(&query, limit.unwrap_or(5) as usize))
    }
    #[napi]
    pub fn graph_rag_json(&self, query: String, limit: Option<u32>) -> Result<String> {
        let config = tapirus::GraphRagConfig::default().with_limit(limit.unwrap_or(5) as usize);
        let result = self
            .connection()?
            .graph_rag_query(&query, None, &config)
            .map_err(|e| Error::from_reason(e.to_string()))?;
        serde_json::to_string(&result).map_err(|e| Error::from_reason(e.to_string()))
    }
}
#[napi]
pub fn version() -> String {
    tapirus::VERSION.to_owned()
}

#[napi]
pub struct SessionDatabaseClient {
    inner: tapirus::SessionDatabase,
}
#[napi]
impl SessionDatabaseClient {
    #[napi(constructor)]
    pub fn new(options: TapirusOptions) -> Result<Self> {
        let config = options.configuration()?;
        let inner = if options.db_path == ":memory:" {
            if options.passphrase.is_some() {
                return Err(Error::from_reason(
                    "Encrypted in-memory session databases are unsupported",
                ));
            }
            tapirus::SessionDatabase::open_in_memory_with_config(config)
        } else {
            tapirus::SessionDatabase::open_with_config(&options.db_path, config)
        }
        .map_err(|e| Error::from_reason(e.to_string()))?;
        Ok(Self { inner })
    }
    #[napi]
    pub fn session(&self, busy_timeout_ms: Option<u32>) -> Result<SessionClient> {
        let inner = self
            .inner
            .session_with_timeout(std::time::Duration::from_millis(
                busy_timeout_ms.unwrap_or(0).into(),
            ))
            .map_err(|e| Error::from_reason(e.to_string()))?;
        Ok(SessionClient { inner })
    }
    #[napi]
    pub fn close(&self) -> Result<()> {
        self.inner
            .close()
            .map_err(|e| Error::from_reason(e.to_string()))
    }
}
#[napi]
pub struct SessionClient {
    inner: tapirus::Session,
}
#[napi]
impl SessionClient {
    #[napi]
    pub fn execute(&self, sql: String, params_json: Option<String>) -> Result<u32> {
        let count = self
            .inner
            .execute_with_params(&sql, &parameters(params_json)?)
            .map_err(|e| Error::from_reason(e.to_string()))?;
        u32::try_from(count).map_err(|_| Error::from_reason("Affected row count exceeds u32 range"))
    }
    #[napi]
    pub fn query(
        &self,
        env: Env,
        sql: String,
        params_json: Option<String>,
    ) -> Result<napi::JsObject> {
        let rows = self
            .inner
            .query_with_params(&sql, &parameters(params_json)?)
            .map_err(|e| Error::from_reason(e.to_string()))?;
        rows_to_js(env, rows)
    }
    #[napi]
    pub fn memory_request_json(&self, operation: String, payload: String) -> Result<String> {
        self.inner
            .memory_request_json(&operation, &payload)
            .map_err(|e| Error::from_reason(e.to_string()))
    }
    #[napi]
    pub fn store_memory_json(&self, payload: String) -> Result<f64> {
        let result = self.memory_request_json("store".into(), payload)?;
        serde_json::from_str::<u64>(&result)
            .map(|id| id as f64)
            .map_err(|e| Error::from_reason(e.to_string()))
    }
    #[napi]
    pub fn search_memories_json(&self, payload: String) -> Result<String> {
        self.memory_request_json("search".into(), payload)
    }
    #[napi]
    pub fn forget_memory(&self, id: String) -> Result<bool> {
        let result = self.memory_request_json("forget".into(), id)?;
        serde_json::from_str(&result).map_err(|e| Error::from_reason(e.to_string()))
    }
    #[napi]
    pub fn checkpoint(&self) -> Result<u32> {
        let count = self
            .inner
            .checkpoint()
            .map_err(|e| Error::from_reason(e.to_string()))?;
        u32::try_from(count).map_err(|_| Error::from_reason("Page count exceeds u32 range"))
    }
    #[napi]
    pub fn close(&self) -> Result<()> {
        self.inner
            .close()
            .map_err(|e| Error::from_reason(e.to_string()))
    }
}
