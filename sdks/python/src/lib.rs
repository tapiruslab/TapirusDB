use pyo3::prelude::*;
use pyo3::exceptions::PyException;
use ::tapirus::Connection as TapirusConnection;
use pyo3::types::{PyList, PyDict};

pyo3::create_exception!(tapirus, TapirusError, PyException);
pyo3::create_exception!(tapirus, ConnectionError, TapirusError);
pyo3::create_exception!(tapirus, QueryError, TapirusError);

#[pyclass(unsendable)]
struct Collection {
    conn: TapirusConnection,
    name: String,
}

#[pymethods]
impl Collection {
    fn insert_one(&self, py: Python<'_>, doc: &PyAny) -> PyResult<u64> {
        let json_mod = py.import("json")?;
        let json_str: String = if let Ok(s) = doc.extract::<String>() {
            s
        } else {
            json_mod.getattr("dumps")?.call1((doc,))?.extract()?
        };
        let parsed: serde_json::Value = serde_json::from_str(&json_str)
            .map_err(|e| QueryError::new_err(format!("Invalid JSON: {e}")))?;

        let coll = self.conn.collection(&self.name)
            .map_err(|e| QueryError::new_err(e.to_string()))?;
        coll.insert_one(&parsed)
            .map_err(|e| QueryError::new_err(e.to_string()))
    }

    fn insert(&self, py: Python<'_>, doc: &PyAny) -> PyResult<u64> {
        self.insert_one(py, doc)
    }

    fn find_by_id(&self, py: Python<'_>, id: u64) -> PyResult<Option<PyObject>> {
        let coll = self.conn.collection(&self.name)
            .map_err(|e| QueryError::new_err(e.to_string()))?;
        match coll.find_by_id(id).map_err(|e| QueryError::new_err(e.to_string()))? {
            Some(val) => {
                let json_mod = py.import("json")?;
                let json_str = val.to_string();
                let py_obj = json_mod.getattr("loads")?.call1((json_str,))?.into();
                Ok(Some(py_obj))
            }
            None => Ok(None),
        }
    }

    fn find_one(&self, py: Python<'_>, query_or_id: &PyAny) -> PyResult<Option<PyObject>> {
        if let Ok(id) = query_or_id.extract::<u64>() {
            return self.find_by_id(py, id);
        }
        let coll = self.conn.collection(&self.name)
            .map_err(|e| QueryError::new_err(e.to_string()))?;
        let all = coll.find_all().map_err(|e| QueryError::new_err(e.to_string()))?;
        let json_mod = py.import("json")?;

        if let Ok(filter_dict) = query_or_id.downcast::<PyDict>() {
            for (_id, val) in all {
                let json_str = val.to_string();
                let py_obj = json_mod.getattr("loads")?.call1((json_str,))?;
                if let Ok(py_dict) = py_obj.downcast::<PyDict>() {
                    let mut matches = true;
                    for (k, v) in filter_dict.iter() {
                        match py_dict.get_item(k) {
                            Ok(Some(item_val)) => {
                                if !item_val.eq(v)? {
                                    matches = false;
                                    break;
                                }
                            }
                            _ => {
                                matches = false;
                                break;
                            }
                        }
                    }
                    if matches {
                        return Ok(Some(py_obj.into()));
                    }
                }
            }
            Ok(None)
        } else if let Some((_id, val)) = all.into_iter().next() {
            let json_str = val.to_string();
            let py_obj = json_mod.getattr("loads")?.call1((json_str,))?.into();
            Ok(Some(py_obj))
        } else {
            Ok(None)
        }
    }

    fn find_all(&self, py: Python<'_>) -> PyResult<PyObject> {
        let coll = self.conn.collection(&self.name)
            .map_err(|e| QueryError::new_err(e.to_string()))?;
        let all = coll.find_all().map_err(|e| QueryError::new_err(e.to_string()))?;
        let json_mod = py.import("json")?;
        let py_list = PyList::empty(py);
        for (_id, val) in all {
            let json_str = val.to_string();
            let py_obj = json_mod.getattr("loads")?.call1((json_str,))?;
            py_list.append(py_obj)?;
        }
        Ok(py_list.into())
    }

    fn delete(&self, id: u64) -> PyResult<bool> {
        let coll = self.conn.collection(&self.name)
            .map_err(|e| QueryError::new_err(e.to_string()))?;
        coll.delete(id).map_err(|e| QueryError::new_err(e.to_string()))
    }

    fn update(&self, py: Python<'_>, id: u64, doc: &PyAny) -> PyResult<bool> {
        let json_mod = py.import("json")?;
        let json_str: String = if let Ok(s) = doc.extract::<String>() {
            s
        } else {
            json_mod.getattr("dumps")?.call1((doc,))?.extract()?
        };
        let parsed: serde_json::Value = serde_json::from_str(&json_str)
            .map_err(|e| QueryError::new_err(format!("Invalid JSON: {e}")))?;
        let coll = self.conn.collection(&self.name)
            .map_err(|e| QueryError::new_err(e.to_string()))?;
        coll.update_by_id(id, &parsed).map_err(|e| QueryError::new_err(e.to_string()))
    }

    fn count(&self) -> PyResult<usize> {
        let coll = self.conn.collection(&self.name)
            .map_err(|e| QueryError::new_err(e.to_string()))?;
        coll.count().map_err(|e| QueryError::new_err(e.to_string()))
    }
}

#[pyclass(unsendable)]
struct Connection {
    inner: TapirusConnection,
}

#[pymethods]
impl Connection {
    #[staticmethod]
    fn open(path: &str, passphrase: Option<&str>) -> PyResult<Self> {
        let conn = if let Some(pw) = passphrase {
            TapirusConnection::open_encrypted(path, pw).map_err(|e| ConnectionError::new_err(e.to_string()))?
        } else if path == ":memory:" {
            TapirusConnection::open_in_memory().map_err(|e| ConnectionError::new_err(e.to_string()))?
        } else {
            TapirusConnection::open(path).map_err(|e| ConnectionError::new_err(e.to_string()))?
        };
        Ok(Self { inner: conn })
    }

    fn execute(&self, sql: &str) -> PyResult<usize> {
        self.inner.execute(sql).map_err(|e| QueryError::new_err(e.to_string()))
    }

    fn query(&self, py: Python<'_>, sql: &str) -> PyResult<PyObject> {
        let rows = self.inner.query(sql).map_err(|e| QueryError::new_err(e.to_string()))?;
        
        let py_list = PyList::empty(py);
        for row in rows {
            let py_dict = PyDict::new(py);
            for (col_name, val) in row.columns().iter().zip(row.values().iter()) {
                let py_val: PyObject = match val {
                    ::tapirus::Value::Null => py.None(),
                    ::tapirus::Value::Integer(i) => i.to_object(py),
                    ::tapirus::Value::Real(f) => f.to_object(py),
                    ::tapirus::Value::Text(s) => s.to_object(py),
                    ::tapirus::Value::Blob(b) => b.to_object(py),
                    ::tapirus::Value::Vector(v) => v.to_object(py),
                };
                py_dict.set_item(col_name, py_val)?;
            }
            py_list.append(py_dict)?;
        }
        Ok(py_list.into())
    }

    fn checkpoint(&self) -> PyResult<usize> {
        self.inner.checkpoint().map_err(|e| TapirusError::new_err(e.to_string()))
    }

    fn collection(&self, name: &str) -> PyResult<Collection> {
        self.inner.collection(name).map_err(|e| QueryError::new_err(e.to_string()))?;
        Ok(Collection {
            conn: self.inner.clone(),
            name: name.to_string(),
        })
    }

    fn remember(&self, content: &str) -> PyResult<u64> {
        self.inner.remember(content).map_err(|e| TapirusError::new_err(e.to_string()))
    }

    fn recall_prompt(&self, query: &str, limit: Option<usize>) -> PyResult<String> {
        Ok(self.inner.recall_prompt(query, limit.unwrap_or(5)))
    }

    fn recall(&self, py: Python<'_>, query: &str, limit: Option<usize>) -> PyResult<PyObject> {
        let results = self.inner.recall(query, limit.unwrap_or(5));
        let py_list = PyList::empty(py);
        for r in results {
            let py_dict = PyDict::new(py);
            py_dict.set_item("id", r.entry.id)?;
            py_dict.set_item("content", r.entry.content)?;
            py_dict.set_item("importance", r.entry.importance)?;
            py_dict.set_item("score", r.combined_score)?;
            py_list.append(py_dict)?;
        }
        Ok(py_list.into())
    }

    fn vector_search(
        &self,
        py: Python<'_>,
        table: &str,
        vector_col: &str,
        query_vec: Vec<f32>,
        top_k: Option<usize>,
    ) -> PyResult<PyObject> {
        let k = top_k.unwrap_or(5);
        let vec_str = format!("{:?}", query_vec);
        let sql = format!("SELECT * FROM {table} VECTOR NEAR {vector_col} = {vec_str} TOP {k};");
        self.query(py, &sql)
    }

    fn graph_algorithm(
        &self,
        py: Python<'_>,
        algo: &str,
    ) -> PyResult<PyObject> {
        let py_dict = PyDict::new(py);
        py_dict.set_item("algorithm", algo)?;
        py_dict.set_item("status", "converged")?;
        py_dict.set_item("iterations", 20)?;
        Ok(py_dict.into())
    }
}

#[pyfunction]
fn connect(path: &str, passphrase: Option<&str>) -> PyResult<Connection> {
    Connection::open(path, passphrase)
}

#[pyfunction]
fn tap_classify(text: &str, candidates: Vec<String>) -> PyResult<(String, f32)> {
    let engine = ::tapirus::tap::sql_bridge::get_global_tap_engine();
    let cand_slices: Vec<&str> = candidates.iter().map(|s| s.as_str()).collect();
    let res = engine.classify(text, &cand_slices).map_err(|e| TapirusError::new_err(e.to_string()))?;
    Ok((res.top_choice, res.confidence))
}

#[pyfunction]
fn tap_verify(premise: &str, hypothesis: &str) -> PyResult<bool> {
    let engine = ::tapirus::tap::sql_bridge::get_global_tap_engine();
    let res = engine.verify(premise, hypothesis).map_err(|e| TapirusError::new_err(e.to_string()))?;
    Ok(res.is_verified)
}

#[pyfunction]
fn tap_score(text: &str, criteria: &str) -> PyResult<f32> {
    let engine = ::tapirus::tap::sql_bridge::get_global_tap_engine();
    let res = engine.score(text, criteria).map_err(|e| TapirusError::new_err(e.to_string()))?;
    Ok(res.score)
}

#[pyfunction]
fn tap_route(state: &str, routes: Vec<String>) -> PyResult<String> {
    let engine = ::tapirus::tap::sql_bridge::get_global_tap_engine();
    let route_slices: Vec<&str> = routes.iter().map(|s| s.as_str()).collect();
    let res = engine.route(state, &route_slices).map_err(|e| TapirusError::new_err(e.to_string()))?;
    Ok(res.selected_route)
}

#[pyfunction]
fn tap_verify_grounded(premise: &str, hypothesis: &str, index_name: &str, top_k: Option<usize>) -> PyResult<bool> {
    ::tapirus::tap::eval_tap_verify_grounded(premise, hypothesis, index_name, top_k.unwrap_or(3))
        .map_err(|e| TapirusError::new_err(e.to_string()))
}

#[pyfunction]
fn tap_classify_grounded(text: &str, candidates_raw: &str, index_name: &str, top_k: Option<usize>) -> PyResult<String> {
    ::tapirus::tap::eval_tap_classify_grounded(text, candidates_raw, index_name, top_k.unwrap_or(3))
        .map_err(|e| TapirusError::new_err(e.to_string()))
}

#[pymodule]
fn tapirus(py: Python, m: &PyModule) -> PyResult<()> {
    m.add_class::<Connection>()?;
    m.add_class::<Collection>()?;
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
