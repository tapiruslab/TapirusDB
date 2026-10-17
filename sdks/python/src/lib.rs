use ::tapirus::{Connection as NativeConnection, MemoryDocument, MemorySearch, Row, Value};
use pyo3::exceptions::{PyException, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyBool, PyBytes, PyDict, PyFloat, PyInt, PyList, PyString, PyTuple};
use std::{cell::RefCell, rc::Rc};

pyo3::create_exception!(tapirus, TapirusError, PyException);
pyo3::create_exception!(tapirus, ConnectionError, TapirusError);
pyo3::create_exception!(tapirus, QueryError, TapirusError);

// Python objects stay on their creating thread. Collections share parent state:
// close releases the file even while collection objects still exist.
type SharedConnection = Rc<RefCell<Option<NativeConnection>>>;
fn with_connection<T>(
    shared: &SharedConnection,
    f: impl FnOnce(&NativeConnection) -> ::tapirus::Result<T>,
) -> PyResult<T> {
    let guard = shared.borrow();
    let conn = guard
        .as_ref()
        .ok_or_else(|| ConnectionError::new_err("Database connection is closed"))?;
    f(conn).map_err(|e| QueryError::new_err(e.to_string()))
}
fn json_text(py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<String> {
    py.import("json")?
        .call_method1("dumps", (value,))?
        .extract()
}
fn json_object(py: Python<'_>, value: &impl serde::Serialize) -> PyResult<Py<PyAny>> {
    let text = serde_json::to_string(value).map_err(|e| QueryError::new_err(e.to_string()))?;
    Ok(py.import("json")?.call_method1("loads", (text,))?.unbind())
}
fn document_json(py: Python<'_>, value: &Bound<'_, PyAny>) -> PyResult<serde_json::Value> {
    let text = if value.is_instance_of::<PyString>() {
        value.extract()?
    } else {
        json_text(py, value)?
    };
    serde_json::from_str(&text).map_err(|e| QueryError::new_err(format!("Invalid JSON: {e}")))
}
fn open_config(
    py: Python<'_>,
    passphrase: Option<&str>,
    memory_config: Option<&Bound<'_, PyAny>>,
) -> PyResult<::tapirus::Config> {
    Ok(::tapirus::Config {
        passphrase: passphrase.map(str::to_owned),
        memory: memory_config
            .map(|value| json_text(py, value))
            .transpose()?
            .map(|text| serde_json::from_str(&text))
            .transpose()
            .map_err(|e| PyValueError::new_err(format!("Invalid memory configuration: {e}")))?
            .unwrap_or_default(),
        ..Default::default()
    })
}
fn parameters(params: Option<&Bound<'_, PyAny>>) -> PyResult<Vec<Value>> {
    let Some(params) = params else {
        return Ok(Vec::new());
    };
    if !params.is_instance_of::<PyList>() && !params.is_instance_of::<PyTuple>() {
        return Err(PyValueError::new_err("Parameters must be a list or tuple"));
    }
    params
        .try_iter()?
        .map(|item| {
            let item = item?;
            Ok(if item.is_none() {
                Value::Null
            } else if item.is_instance_of::<PyBool>() {
                Value::Integer(i64::from(item.extract::<bool>()?))
            } else if item.is_instance_of::<PyInt>() {
                Value::Integer(item.extract::<i64>()?)
            } else if item.is_instance_of::<PyFloat>() {
                let value: f64 = item.extract()?;
                if !value.is_finite() {
                    return Err(PyValueError::new_err("SQL real parameters must be finite"));
                }
                Value::Real(value)
            } else if item.is_instance_of::<PyString>() {
                Value::Text(item.extract()?)
            } else if item.is_instance_of::<PyBytes>() {
                Value::Blob(item.extract()?)
            } else if item.is_instance_of::<PyList>() || item.is_instance_of::<PyTuple>() {
                let vector: Vec<f32> = item.extract()?;
                if vector.is_empty() || vector.iter().any(|v| !v.is_finite()) {
                    return Err(PyValueError::new_err(
                        "Vector parameters must be nonempty and finite",
                    ));
                }
                Value::Vector(vector)
            } else {
                return Err(PyValueError::new_err("Unsupported SQL parameter type"));
            })
        })
        .collect()
}
fn rows_to_python(py: Python<'_>, rows: Vec<Row>) -> PyResult<Py<PyAny>> {
    let list = PyList::empty(py);
    for row in rows {
        let dict = PyDict::new(py);
        for (name, value) in row.columns().iter().zip(row.values()) {
            match value {
                Value::Null => dict.set_item(name, py.None())?,
                Value::Integer(v) => dict.set_item(name, v)?,
                Value::Real(v) => dict.set_item(name, v)?,
                Value::Text(v) => dict.set_item(name, v)?,
                Value::Blob(v) => dict.set_item(name, PyBytes::new(py, v))?,
                Value::Vector(v) => dict.set_item(name, v)?,
            }
        }
        list.append(dict)?;
    }
    Ok(list.into_any().unbind())
}

#[pyclass(unsendable)]
struct Collection {
    conn: SharedConnection,
    name: String,
}
#[pymethods]
impl Collection {
    fn insert_one(&self, py: Python<'_>, doc: &Bound<'_, PyAny>) -> PyResult<u64> {
        let parsed = document_json(py, doc)?;
        with_connection(&self.conn, |conn| {
            conn.collection(&self.name)?.insert_one(&parsed)
        })
    }
    fn insert(&self, py: Python<'_>, doc: &Bound<'_, PyAny>) -> PyResult<u64> {
        self.insert_one(py, doc)
    }
    fn find_by_id(&self, py: Python<'_>, id: u64) -> PyResult<Option<Py<PyAny>>> {
        with_connection(&self.conn, |conn| {
            conn.collection(&self.name)?.find_by_id(id)
        })?
        .map(|v| json_object(py, &v))
        .transpose()
    }
    fn find_one(
        &self,
        py: Python<'_>,
        query_or_id: &Bound<'_, PyAny>,
    ) -> PyResult<Option<Py<PyAny>>> {
        if let Ok(id) = query_or_id.extract::<u64>() {
            return self.find_by_id(py, id);
        }
        let filter = document_json(py, query_or_id)?;
        let filter = filter
            .as_object()
            .ok_or_else(|| PyValueError::new_err("Filter must be a dictionary or numeric ID"))?;
        let all = with_connection(&self.conn, |conn| conn.collection(&self.name)?.find_all())?;
        all.into_iter()
            .find(|(_, value)| {
                filter
                    .iter()
                    .all(|(key, expected)| value.get(key) == Some(expected))
            })
            .map(|(_, v)| json_object(py, &v))
            .transpose()
    }
    fn find_all(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let all = with_connection(&self.conn, |conn| conn.collection(&self.name)?.find_all())?;
        json_object(
            py,
            &all.into_iter().map(|(_, value)| value).collect::<Vec<_>>(),
        )
    }
    fn delete(&self, id: u64) -> PyResult<bool> {
        with_connection(&self.conn, |conn| conn.collection(&self.name)?.delete(id))
    }
    fn update(&self, py: Python<'_>, id: u64, doc: &Bound<'_, PyAny>) -> PyResult<bool> {
        let parsed = document_json(py, doc)?;
        with_connection(&self.conn, |conn| {
            conn.collection(&self.name)?.update_by_id(id, &parsed)
        })
    }
    fn count(&self) -> PyResult<usize> {
        with_connection(&self.conn, |conn| conn.collection(&self.name)?.count())
    }
}

#[pyclass(unsendable)]
struct Connection {
    inner: SharedConnection,
}
#[pymethods]
impl Connection {
    #[staticmethod]
    #[pyo3(signature = (path, passphrase=None, *, memory_config=None))]
    fn open(
        py: Python<'_>,
        path: &str,
        passphrase: Option<&str>,
        memory_config: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        let config = open_config(py, passphrase, memory_config)?;
        let conn = if path == ":memory:" {
            NativeConnection::open_in_memory_with_config(config)
        } else {
            NativeConnection::open_with_config(path, config)
        }
        .map_err(|e| ConnectionError::new_err(e.to_string()))?;
        Ok(Self {
            inner: Rc::new(RefCell::new(Some(conn))),
        })
    }
    #[pyo3(signature = (sql, params=None))]
    fn execute(&self, sql: &str, params: Option<&Bound<'_, PyAny>>) -> PyResult<usize> {
        let params = parameters(params)?;
        with_connection(&self.inner, |conn| conn.execute_with_params(sql, &params))
    }
    #[pyo3(signature = (sql, params=None))]
    fn query(
        &self,
        py: Python<'_>,
        sql: &str,
        params: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        let params = parameters(params)?;
        rows_to_python(
            py,
            with_connection(&self.inner, |conn| conn.query_with_params(sql, &params))?,
        )
    }
    fn checkpoint(&self) -> PyResult<usize> {
        with_connection(&self.inner, NativeConnection::checkpoint)
    }
    fn schema(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        json_object(py, &with_connection(&self.inner, |conn| Ok(conn.tables()))?)
    }
    fn integrity_check(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        json_object(
            py,
            &with_connection(&self.inner, NativeConnection::check_integrity)?,
        )
    }
    fn close(&self) -> PyResult<()> {
        let mut guard = self.inner.borrow_mut();
        if let Some(conn) = guard.as_ref() {
            conn.close()
                .map_err(|e| ConnectionError::new_err(e.to_string()))?;
        }
        guard.take();
        Ok(())
    }
    fn __enter__(slf: PyRef<'_, Self>) -> PyResult<PyRef<'_, Self>> {
        with_connection(&slf.inner, |_| Ok(()))?;
        Ok(slf)
    }
    fn __exit__(
        &self,
        _ty: &Bound<'_, PyAny>,
        _value: &Bound<'_, PyAny>,
        _traceback: &Bound<'_, PyAny>,
    ) -> PyResult<bool> {
        self.close()?;
        Ok(false)
    }
    fn collection(&self, name: &str) -> PyResult<Collection> {
        with_connection(&self.inner, |conn| conn.collection(name).map(|_| ()))?;
        Ok(Collection {
            conn: self.inner.clone(),
            name: name.to_string(),
        })
    }
    fn remember(&self, content: &str) -> PyResult<u64> {
        with_connection(&self.inner, |conn| conn.remember(content))
    }
    #[pyo3(signature = (query, limit=5))]
    fn recall_prompt(&self, query: &str, limit: usize) -> PyResult<String> {
        with_connection(&self.inner, |conn| Ok(conn.recall_prompt(query, limit)))
    }
    #[pyo3(signature = (query, limit=5))]
    fn recall(&self, py: Python<'_>, query: &str, limit: usize) -> PyResult<Py<PyAny>> {
        let results = with_connection(&self.inner, |conn| Ok(conn.recall(query, limit)))?;
        let list = PyList::empty(py);
        for result in results {
            let dict = PyDict::new(py);
            dict.set_item("id", result.entry.id)?;
            dict.set_item("content", result.entry.content)?;
            dict.set_item("importance", result.entry.importance)?;
            dict.set_item("score", result.combined_score)?;
            list.append(dict)?;
        }
        Ok(list.into_any().unbind())
    }
    fn store_memory(&self, py: Python<'_>, document: &Bound<'_, PyAny>) -> PyResult<u64> {
        let request: MemoryDocument = serde_json::from_str(&json_text(py, document)?)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        with_connection(&self.inner, |conn| conn.store_memory(&request))
    }
    fn search_memories(&self, py: Python<'_>, search: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        let request: MemorySearch = serde_json::from_str(&json_text(py, search)?)
            .map_err(|e| PyValueError::new_err(e.to_string()))?;
        json_object(
            py,
            &with_connection(&self.inner, |conn| conn.search_memories(&request))?,
        )
    }
    fn forget_memory(&self, id: u64) -> PyResult<bool> {
        with_connection(&self.inner, |conn| conn.memory_forget_checked(id))
    }
    fn upsert_memory(&self, py: Python<'_>, document: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        self.memory_request(py, "upsert", Some(document))
    }
    fn store_memory_batch(
        &self,
        py: Python<'_>,
        documents: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        self.memory_request(py, "store_batch", Some(documents))
    }
    fn upsert_memory_batch(
        &self,
        py: Python<'_>,
        documents: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        self.memory_request(py, "upsert_batch", Some(documents))
    }
    fn search_memories_detailed(
        &self,
        py: Python<'_>,
        search: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        self.memory_request(py, "search_detailed", Some(search))
    }
    fn configure_memory(&self, py: Python<'_>, config: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        self.memory_request(py, "configure", Some(config))
    }
    fn memory_stats(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.memory_request(py, "stats", None)
    }
    fn storage_health(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.memory_request(py, "storage_health", None)
    }
    fn prepare_memory_indexes(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.memory_request(py, "prepare", None)
    }
    fn purge_expired_memories(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.memory_request(py, "purge_expired", None)
    }
    fn delete_memories(&self, py: Python<'_>, scope: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        self.memory_request(py, "delete", Some(scope))
    }
    #[pyo3(signature = (table, vector_col, query_vec, top_k=5))]
    fn vector_search(
        &self,
        py: Python<'_>,
        table: &str,
        vector_col: &str,
        query_vec: Vec<f32>,
        top_k: usize,
    ) -> PyResult<Py<PyAny>> {
        for identifier in [table, vector_col] {
            if identifier.is_empty()
                || !identifier.bytes().enumerate().all(|(i, c)| {
                    c == b'_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit())
                })
            {
                return Err(PyValueError::new_err("Expected a plain SQL identifier"));
            }
        }
        if top_k > 10_000 || query_vec.is_empty() || query_vec.iter().any(|v| !v.is_finite()) {
            return Err(PyValueError::new_err("Invalid vector or top_k"));
        }
        let sql = format!("SELECT * FROM {table} VECTOR NEAR {vector_col} = ? TOP {top_k}");
        rows_to_python(
            py,
            with_connection(&self.inner, |conn| {
                conn.query_with_params(&sql, &[Value::Vector(query_vec)])
            })?,
        )
    }
    fn graph_algorithm(&self, py: Python<'_>, algo: &str) -> PyResult<Py<PyAny>> {
        if algo.is_empty() || !algo.bytes().all(|c| c.is_ascii_alphabetic() || c == b'_') {
            return Err(PyValueError::new_err("Expected a graph algorithm name"));
        }
        self.query(
            py,
            &format!("GRAPH ALGORITHM {}", algo.to_uppercase()),
            None,
        )
    }
}

impl Connection {
    fn memory_request(
        &self,
        py: Python<'_>,
        operation: &str,
        payload: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        let payload = payload
            .map(|value| json_text(py, value))
            .transpose()?
            .unwrap_or_else(|| "{}".into());
        let result = with_connection(&self.inner, |conn| {
            conn.memory_request_json(operation, &payload)
        })?;
        Ok(py
            .import("json")?
            .call_method1("loads", (result,))?
            .unbind())
    }
}

#[pyclass(unsendable)]
struct SessionDatabase {
    inner: ::tapirus::SessionDatabase,
}
#[pymethods]
impl SessionDatabase {
    #[staticmethod]
    #[pyo3(signature = (path=":memory:", passphrase=None, *, memory_config=None))]
    fn open(
        py: Python<'_>,
        path: &str,
        passphrase: Option<&str>,
        memory_config: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Self> {
        let config = open_config(py, passphrase, memory_config)?;
        let inner = if path == ":memory:" {
            ::tapirus::SessionDatabase::open_in_memory_with_config(config)
        } else {
            ::tapirus::SessionDatabase::open_with_config(path, config)
        }
        .map_err(|e| ConnectionError::new_err(e.to_string()))?;
        Ok(Self { inner })
    }
    #[pyo3(signature = (busy_timeout_ms=0))]
    fn session(&self, busy_timeout_ms: u32) -> PyResult<SessionConnection> {
        Ok(SessionConnection {
            inner: self
                .inner
                .session_with_timeout(std::time::Duration::from_millis(u64::from(busy_timeout_ms)))
                .map_err(|e| ConnectionError::new_err(e.to_string()))?,
        })
    }
    fn close(&self) -> PyResult<()> {
        self.inner
            .close()
            .map_err(|e| ConnectionError::new_err(e.to_string()))
    }
}
#[pyclass(name = "Session", unsendable)]
struct SessionConnection {
    inner: ::tapirus::Session,
}
#[pymethods]
impl SessionConnection {
    #[pyo3(signature = (sql, params=None))]
    fn execute(&self, sql: &str, params: Option<&Bound<'_, PyAny>>) -> PyResult<usize> {
        self.inner
            .execute_with_params(sql, &parameters(params)?)
            .map_err(|e| QueryError::new_err(e.to_string()))
    }
    #[pyo3(signature = (sql, params=None))]
    fn query(
        &self,
        py: Python<'_>,
        sql: &str,
        params: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        rows_to_python(
            py,
            self.inner
                .query_with_params(sql, &parameters(params)?)
                .map_err(|e| QueryError::new_err(e.to_string()))?,
        )
    }
    fn begin(&self) -> PyResult<()> {
        self.execute("BEGIN", None).map(|_| ())
    }
    fn commit(&self) -> PyResult<()> {
        self.execute("COMMIT", None).map(|_| ())
    }
    fn rollback(&self) -> PyResult<()> {
        self.execute("ROLLBACK", None).map(|_| ())
    }
    fn checkpoint(&self) -> PyResult<usize> {
        self.inner
            .checkpoint()
            .map_err(|e| QueryError::new_err(e.to_string()))
    }
    fn close(&self) -> PyResult<()> {
        self.inner
            .close()
            .map_err(|e| ConnectionError::new_err(e.to_string()))
    }
    fn __enter__(slf: PyRef<'_, Self>) -> PyResult<PyRef<'_, Self>> {
        slf.begin()?;
        Ok(slf)
    }
    fn __exit__(
        &self,
        ty: &Bound<'_, PyAny>,
        _value: &Bound<'_, PyAny>,
        _traceback: &Bound<'_, PyAny>,
    ) -> PyResult<bool> {
        if ty.is_none() {
            self.commit()?;
        } else {
            self.rollback()?;
        }
        Ok(false)
    }
    fn store_memory(&self, py: Python<'_>, document: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        self.memory_request(py, "store", Some(document))
    }
    fn search_memories(&self, py: Python<'_>, search: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        self.memory_request(py, "search", Some(search))
    }
    fn forget_memory(&self, id: u64) -> PyResult<bool> {
        let result = self
            .inner
            .memory_request_json("forget", &id.to_string())
            .map_err(|e| QueryError::new_err(e.to_string()))?;
        serde_json::from_str(&result).map_err(|e| QueryError::new_err(e.to_string()))
    }
    fn upsert_memory(&self, py: Python<'_>, document: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        self.memory_request(py, "upsert", Some(document))
    }
    fn store_memory_batch(
        &self,
        py: Python<'_>,
        documents: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        self.memory_request(py, "store_batch", Some(documents))
    }
    fn upsert_memory_batch(
        &self,
        py: Python<'_>,
        documents: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        self.memory_request(py, "upsert_batch", Some(documents))
    }
    fn search_memories_detailed(
        &self,
        py: Python<'_>,
        search: &Bound<'_, PyAny>,
    ) -> PyResult<Py<PyAny>> {
        self.memory_request(py, "search_detailed", Some(search))
    }
    fn configure_memory(&self, py: Python<'_>, config: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        self.memory_request(py, "configure", Some(config))
    }
    fn memory_stats(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.memory_request(py, "stats", None)
    }
    fn storage_health(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.memory_request(py, "storage_health", None)
    }
    fn prepare_memory_indexes(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.memory_request(py, "prepare", None)
    }
    fn purge_expired_memories(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        self.memory_request(py, "purge_expired", None)
    }
    fn delete_memories(&self, py: Python<'_>, scope: &Bound<'_, PyAny>) -> PyResult<Py<PyAny>> {
        self.memory_request(py, "delete", Some(scope))
    }
}
impl SessionConnection {
    fn memory_request(
        &self,
        py: Python<'_>,
        operation: &str,
        payload: Option<&Bound<'_, PyAny>>,
    ) -> PyResult<Py<PyAny>> {
        let payload = payload
            .map(|value| json_text(py, value))
            .transpose()?
            .unwrap_or_else(|| "{}".into());
        let result = self
            .inner
            .memory_request_json(operation, &payload)
            .map_err(|e| QueryError::new_err(e.to_string()))?;
        Ok(py
            .import("json")?
            .call_method1("loads", (result,))?
            .unbind())
    }
}

#[pyfunction]
#[pyo3(signature = (path, passphrase=None, *, memory_config=None))]
fn connect(
    py: Python<'_>,
    path: &str,
    passphrase: Option<&str>,
    memory_config: Option<&Bound<'_, PyAny>>,
) -> PyResult<Connection> {
    Connection::open(py, path, passphrase, memory_config)
}

#[pyfunction]
fn tap_classify(text: &str, candidates: Vec<String>) -> PyResult<(String, f32)> {
    let choices: Vec<_> = candidates.iter().map(String::as_str).collect();
    let result = ::tapirus::tap::sql_bridge::get_global_tap_engine()
        .classify(text, &choices)
        .map_err(|e| TapirusError::new_err(e.to_string()))?;
    Ok((result.top_choice, result.confidence))
}
#[pyfunction]
fn tap_verify(premise: &str, hypothesis: &str) -> PyResult<bool> {
    ::tapirus::tap::sql_bridge::get_global_tap_engine()
        .verify(premise, hypothesis)
        .map(|r| r.is_verified)
        .map_err(|e| TapirusError::new_err(e.to_string()))
}
#[pyfunction]
fn tap_score(text: &str, criteria: &str) -> PyResult<f32> {
    ::tapirus::tap::sql_bridge::get_global_tap_engine()
        .score(text, criteria)
        .map(|r| r.score)
        .map_err(|e| TapirusError::new_err(e.to_string()))
}
#[pyfunction]
fn tap_route(state: &str, routes: Vec<String>) -> PyResult<String> {
    let choices: Vec<_> = routes.iter().map(String::as_str).collect();
    ::tapirus::tap::sql_bridge::get_global_tap_engine()
        .route(state, &choices)
        .map(|r| r.selected_route)
        .map_err(|e| TapirusError::new_err(e.to_string()))
}
#[pyfunction]
#[pyo3(signature = (premise, hypothesis, index_name, top_k=3))]
fn tap_verify_grounded(
    premise: &str,
    hypothesis: &str,
    index_name: &str,
    top_k: usize,
) -> PyResult<bool> {
    ::tapirus::tap::eval_tap_verify_grounded(premise, hypothesis, index_name, top_k)
        .map_err(|e| TapirusError::new_err(e.to_string()))
}
#[pyfunction]
#[pyo3(signature = (text, candidates_raw, index_name, top_k=3))]
fn tap_classify_grounded(
    text: &str,
    candidates_raw: &str,
    index_name: &str,
    top_k: usize,
) -> PyResult<String> {
    ::tapirus::tap::eval_tap_classify_grounded(text, candidates_raw, index_name, top_k)
        .map_err(|e| TapirusError::new_err(e.to_string()))
}

#[pymodule(gil_used = true)]
fn tapirus(py: Python<'_>, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", ::tapirus::VERSION)?;
    m.add_class::<Connection>()?;
    m.add_class::<Collection>()?;
    m.add_class::<SessionDatabase>()?;
    m.add_class::<SessionConnection>()?;
    m.add("Tapirus", py.get_type::<Connection>())?;
    m.add_function(wrap_pyfunction!(connect, m)?)?;
    m.add_function(wrap_pyfunction!(tap_classify, m)?)?;
    m.add_function(wrap_pyfunction!(tap_classify_grounded, m)?)?;
    m.add_function(wrap_pyfunction!(tap_verify, m)?)?;
    m.add_function(wrap_pyfunction!(tap_verify_grounded, m)?)?;
    m.add_function(wrap_pyfunction!(tap_score, m)?)?;
    m.add_function(wrap_pyfunction!(tap_route, m)?)?;
    m.add("TapirusError", py.get_type::<TapirusError>())?;
    m.add("ConnectionError", py.get_type::<ConnectionError>())?;
    m.add("QueryError", py.get_type::<QueryError>())?;
    Ok(())
}
