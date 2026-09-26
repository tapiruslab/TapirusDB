#![deny(clippy::all)]

use napi::bindgen_prelude::*;
use napi_derive::napi;
use tapirus::Connection as TapirusConnection;
use tapirus::Value;

#[napi]
pub struct TapirusClient {
    inner: TapirusConnection,
}

#[napi(object)]
pub struct TapirusOptions {
    pub db_path: String,
    pub passphrase: Option<String>,
}

#[napi]
impl TapirusClient {
    #[napi(constructor)]
    pub fn new(options: TapirusOptions) -> Result<Self> {
        let conn = if let Some(pw) = options.passphrase {
            TapirusConnection::open_encrypted(&options.db_path, &pw)
                .map_err(|e| Error::from_reason(e.to_string()))?
        } else if options.db_path == ":memory:" {
            TapirusConnection::open_in_memory()
                .map_err(|e| Error::from_reason(e.to_string()))?
        } else {
            TapirusConnection::open(&options.db_path)
                .map_err(|e| Error::from_reason(e.to_string()))?
        };
        Ok(Self { inner: conn })
    }

    #[napi]
    pub fn execute(&self, sql: String) -> Result<u32> {
        let rows_affected = self.inner.execute(&sql).map_err(|e| Error::from_reason(e.to_string()))?;
        Ok(rows_affected as u32)
    }

    #[napi]
    pub fn query(&self, env: Env, sql: String) -> Result<napi::JsObject> {
        let rows = self.inner.query(&sql).map_err(|e| Error::from_reason(e.to_string()))?;
        let mut arr = env.create_array_with_length(rows.len())?;
        
        for (i, row) in rows.into_iter().enumerate() {
            let mut obj = env.create_object()?;
            for (col_name, val) in row.columns().iter().zip(row.values().iter()) {
                match val {
                    Value::Null => obj.set(col_name, env.get_null()?)?,
                    Value::Integer(v) => obj.set(col_name, env.create_int64(*v)?)?,
                    Value::Real(v) => obj.set(col_name, env.create_double(*v)?)?,
                    Value::Text(v) => obj.set(col_name, env.create_string(&v)?)?,
                    Value::Blob(v) => {
                        let buf = env.create_buffer_with_data(v.clone())?.into_raw();
                        obj.set(col_name, buf)?
                    },
                    Value::Vector(v) => {
                        let mut vec_arr = env.create_array_with_length(v.len())?;
                        for (j, float_val) in v.iter().enumerate() {
                            vec_arr.set_element(j as u32, env.create_double(*float_val as f64)?)?;
                        }
                        obj.set(col_name, vec_arr)?
                    }
                }
            }
            arr.set_element(i as u32, obj)?;
        }
        
        Ok(arr)
    }

    #[napi]
    pub fn checkpoint(&self) -> Result<u32> {
        let count = self.inner.checkpoint().map_err(|e| Error::from_reason(e.to_string()))?;
        Ok(count as u32)
    }

    #[napi]
    pub fn close(&self) -> Result<()> {
        Ok(())
    }
}
