use pyo3::prelude::*;
use pyo3::exceptions::PyException;
use ::tapirus::Connection as TapirusConnection;
use pyo3::types::{PyList, PyDict};

pyo3::create_exception!(tapirus, TapirusError, PyException);
pyo3::create_exception!(tapirus, ConnectionError, TapirusError);
pyo3::create_exception!(tapirus, QueryError, TapirusError);

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

#[pymodule]
fn tapirus(py: Python, m: &PyModule) -> PyResult<()> {
    m.add_class::<Connection>()?;
    m.add_function(wrap_pyfunction!(connect, m)?)?;
    m.add_function(wrap_pyfunction!(tap_classify, m)?)?;
    m.add_function(wrap_pyfunction!(tap_verify, m)?)?;
    m.add_function(wrap_pyfunction!(tap_score, m)?)?;
    m.add_function(wrap_pyfunction!(tap_route, m)?)?;
    m.add("TapirusError", py.get_type::<TapirusError>())?;
    m.add("ConnectionError", py.get_type::<ConnectionError>())?;
    m.add("QueryError", py.get_type::<QueryError>())?;
    Ok(())
}
