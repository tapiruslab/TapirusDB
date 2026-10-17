use tapirus::memory::{DeterministicHashEmbedder, EmbeddingProvider, HttpEmbeddingEngine};
use tapirus::{Connection, MemoryDocument};

#[test]
fn fallible_hash_provider_persists_explicit_model_and_rejects_empty_features() {
    let db = Connection::open_in_memory().unwrap();
    let provider = DeterministicHashEmbedder::new(64);
    let document = MemoryDocument {
        content: "Robot maintenance guide".into(),
        external_id: Some("manual".into()),
        ..Default::default()
    };
    let ids = db.upsert_embedded_memories(&provider, &[document]).unwrap();
    assert_eq!(
        db.memory_get(ids[0]).unwrap().embedding_model.as_deref(),
        Some("tapirus/feature-hash-v1/64")
    );
    assert!(provider.embed_documents(&["   "]).is_err());
}

struct InvalidProvider;
impl EmbeddingProvider for InvalidProvider {
    fn model_id(&self) -> String {
        "broken-model".into()
    }
    fn vector_dimensions(&self) -> usize {
        2
    }
    fn embed_documents(&self, _texts: &[&str]) -> tapirus::Result<Vec<Vec<f32>>> {
        Ok(vec![vec![f32::NAN, 1.0]])
    }
}
#[test]
fn malformed_provider_response_and_offline_service_leave_database_unchanged() {
    let db = Connection::open_in_memory().unwrap();
    let document = MemoryDocument {
        content: "A source".into(),
        external_id: Some("a".into()),
        ..Default::default()
    };
    assert!(
        db.upsert_embedded_memories(&InvalidProvider, std::slice::from_ref(&document))
            .is_err()
    );
    let remote = HttpEmbeddingEngine::new("http://127.0.0.1:1/api/embed", "missing-model", 2);
    assert!(db.upsert_embedded_memories(&remote, &[document]).is_err());
    assert_eq!(db.memory_count(), 0);
    let mut authenticated = remote;
    authenticated.api_key = Some("do-not-print-secret".into());
    assert!(!format!("{authenticated:?}").contains("do-not-print-secret"));
}

#[cfg(all(feature = "http-embeddings", not(target_arch = "wasm32")))]
#[test]
fn http_provider_validates_batch_count_dimensions_status_and_input_order() {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        thread,
    };
    for (response, ok) in [
        (r#"{"embeddings":[[1,0],[0,1]]}"#, true),
        (r#"{"embeddings":[[1,0]]}"#, false),
        (r#"{"embeddings":[[1],[0,1]]}"#, false),
        (r#"{"embeddings":[[0,0],[0,1]]}"#, false),
        (
            r#"{"data":[{"index":1,"embedding":[1,0]},{"index":0,"embedding":[0,1]}]}"#,
            false,
        ),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut bytes = Vec::new();
            let mut buffer = [0u8; 1024];
            loop {
                let n = stream.read(&mut buffer).unwrap();
                if n == 0 {
                    break;
                }
                bytes.extend_from_slice(&buffer[..n]);
                if let Some(end) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..end]);
                    let length: usize = headers
                        .lines()
                        .find_map(|line| {
                            line.to_lowercase()
                                .strip_prefix("content-length:")
                                .map(|n| n.trim().parse().unwrap())
                        })
                        .unwrap();
                    if bytes.len() >= end + 4 + length {
                        let body: serde_json::Value =
                            serde_json::from_slice(&bytes[end + 4..end + 4 + length]).unwrap();
                        assert_eq!(body["input"], serde_json::json!(["first", "second"]));
                        assert_eq!(body["truncate"], false);
                        break;
                    }
                }
            }
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response.len(),
                response
            )
            .unwrap();
        });
        let provider =
            HttpEmbeddingEngine::new(&format!("http://{address}/api/embed"), "fixture-model", 2);
        assert_eq!(provider.embed_documents(&["first", "second"]).is_ok(), ok);
        worker.join().unwrap();
    }
}
