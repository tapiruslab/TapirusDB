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

#[pymodule]
fn tapirus(py: Python, m: &PyModule) -> PyResult<()> {
    m.add_class::<Connection>()?;
    m.add_function(wrap_pyfunction!(connect, m)?)?;
    m.add("TapirusError", py.get_type::<TapirusError>())?;
    m.add("ConnectionError", py.get_type::<ConnectionError>())?;
    m.add("QueryError", py.get_type::<QueryError>())?;
    Ok(())
}
