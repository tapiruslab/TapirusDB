//! Edition boundaries and file interoperability through the public APIs.

#[cfg(all(
    feature = "edition-lite",
    feature = "edition-document",
    feature = "edition-graph",
    feature = "edition-vector"
))]
#[test]
fn all_four_editions_reopen_one_file_and_quad_reads_every_model() {
    use serde_json::json;
    use tapirus::editions::{DocumentDatabase, GraphDatabase, LiteDatabase, VectorDatabase};
    use tapirus::{Connection, Direction};

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("quad-interoperability.tapir");

    let lite = LiteDatabase::open(&path).unwrap();
    lite.execute("CREATE TABLE notes (id INTEGER PRIMARY KEY, body TEXT)")
        .unwrap();
    lite.execute("INSERT INTO notes (id, body) VALUES (1, 'shared')")
        .unwrap();
    lite.close().unwrap();
    drop(lite);

    let document = DocumentDatabase::open(&path).unwrap();
    let collection = document.collection("people").unwrap();
    let document_id = collection.insert_one(&json!({"name":"Ada"})).unwrap();
    drop(collection);
    document.close().unwrap();
    drop(document);

    let graph = GraphDatabase::open(&path).unwrap();
    graph.add_node(1, "Person", r#"{"name":"Ada"}"#).unwrap();
    graph
        .add_node(2, "Person", r#"{"name":"Alex Chen"}"#)
        .unwrap();
    graph.add_edge(1, 2, "KNOWS", 1.0, "{}").unwrap();
    graph.close().unwrap();
    drop(graph);

    let vector = VectorDatabase::open(&path).unwrap();
    let collection = vector.collection("people", 2).unwrap();
    collection
        .insert(1, &[1.0, 0.0], &json!({"name":"Ada"}))
        .unwrap();
    drop(collection);
    vector.close().unwrap();
    drop(vector);

    let quad = Connection::open(&path).unwrap();
    assert_eq!(quad.query("SELECT id FROM notes").unwrap().len(), 1);
    assert_eq!(
        quad.collection("people")
            .unwrap()
            .find_by_id(document_id)
            .unwrap(),
        Some(json!({"name":"Ada"}))
    );
    assert_eq!(quad.graph_neighbors(1, Direction::Outgoing, None).len(), 1);
    assert_eq!(quad.query("SELECT id FROM __vec_people").unwrap().len(), 1);
    quad.close().unwrap();
}

#[cfg(all(feature = "edition-lite", feature = "edition-document"))]
#[test]
fn lite_and_document_use_one_native_file_without_cross_model_access() {
    use serde_json::json;
    use tapirus::Connection;
    use tapirus::editions::{DocumentDatabase, LiteDatabase};

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("editions.tapir");

    let lite = LiteDatabase::open(&path).unwrap();
    lite.execute("CREATE TABLE notes (id INTEGER PRIMARY KEY, body TEXT)")
        .unwrap();
    lite.execute_with_params(
        "INSERT INTO notes (id, body) VALUES (?, ?)",
        &[
            tapirus::Value::Integer(1),
            tapirus::Value::Text("hello".into()),
        ],
    )
    .unwrap();
    assert_eq!(
        lite.query("SELECT body FROM notes WHERE id = 1").unwrap()[0]
            .get::<String>("body")
            .unwrap(),
        "hello"
    );
    assert!(
        lite.execute("GRAPH INSERT NODE 1 LABEL Person PROPERTIES '{}'")
            .is_err()
    );
    assert!(
        lite.execute("CREATE TABLE vectors (embedding VECTOR)")
            .is_err()
    );
    assert!(
        lite.query("SELECT TAP_SCORE(body, 'hello') FROM notes")
            .is_err()
    );
    assert!(lite.query("SELECT * FROM __sys_memory").is_err());
    lite.close().unwrap();
    drop(lite);

    let document = DocumentDatabase::open(&path).unwrap();
    let collection = document.collection("users").unwrap();
    let id = collection
        .insert_one(&json!({"name": "Alex Chen"}))
        .unwrap();
    assert_eq!(
        collection.find_by_id(id).unwrap(),
        Some(json!({"name": "Alex Chen"}))
    );
    drop(collection);
    document.close().unwrap();
    drop(document);

    let lite = LiteDatabase::open(&path).unwrap();
    assert!(lite.query("SELECT * FROM __doc_users").is_err());
    assert_eq!(lite.query("SELECT id FROM notes").unwrap().len(), 1);
    lite.close().unwrap();
    drop(lite);

    let quad = Connection::open(&path).unwrap();
    assert_eq!(
        quad.collection("users").unwrap().find_by_id(id).unwrap(),
        Some(json!({"name": "Alex Chen"}))
    );
    assert_eq!(quad.query("SELECT id FROM notes").unwrap().len(), 1);
    quad.close().unwrap();
}

#[cfg(all(feature = "edition-lite", feature = "edition-document"))]
#[test]
fn edition_binaries_share_a_file_across_processes() {
    use std::process::Command;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("processes.tapir");
    let lite = env!("CARGO_BIN_EXE_tapirus-lite");
    let document = env!("CARGO_BIN_EXE_tapirus-document");

    let create = Command::new(lite)
        .args([
            path.as_os_str(),
            "--exec".as_ref(),
            "CREATE TABLE items (id INTEGER PRIMARY KEY, label TEXT)".as_ref(),
        ])
        .output()
        .unwrap();
    assert!(
        create.status.success(),
        "{}",
        String::from_utf8_lossy(&create.stderr)
    );

    let insert = Command::new(document)
        .args([
            path.as_os_str(),
            "people".as_ref(),
            "insert".as_ref(),
            r#"{"name":"Ada"}"#.as_ref(),
        ])
        .output()
        .unwrap();
    assert!(
        insert.status.success(),
        "{}",
        String::from_utf8_lossy(&insert.stderr)
    );

    let query = Command::new(lite)
        .args([
            path.as_os_str(),
            "--query".as_ref(),
            "SELECT id FROM items".as_ref(),
        ])
        .output()
        .unwrap();
    assert!(
        query.status.success(),
        "{}",
        String::from_utf8_lossy(&query.stderr)
    );
    assert_eq!(String::from_utf8(query.stdout).unwrap().trim(), "[]");

    let denied = Command::new(lite)
        .args([
            path.as_os_str(),
            "--query".as_ref(),
            "SELECT * FROM __doc_people".as_ref(),
        ])
        .output()
        .unwrap();
    assert!(!denied.status.success());

    let list = Command::new(document)
        .args([path.as_os_str(), "people".as_ref(), "list".as_ref()])
        .output()
        .unwrap();
    assert!(
        list.status.success(),
        "{}",
        String::from_utf8_lossy(&list.stderr)
    );
    assert!(String::from_utf8(list.stdout).unwrap().contains("Ada"));

    let count = Command::new(document)
        .args([path.as_os_str(), "people".as_ref(), "count".as_ref()])
        .output()
        .unwrap();
    assert!(
        count.status.success(),
        "{}",
        String::from_utf8_lossy(&count.stderr)
    );
    assert_eq!(String::from_utf8(count.stdout).unwrap().trim(), "1");
}

#[cfg(all(feature = "edition-graph", feature = "edition-vector"))]
#[test]
fn graph_and_vector_editions_share_the_quad_file() {
    use serde_json::json;
    use tapirus::editions::{GraphDatabase, VectorDatabase};
    use tapirus::{Connection, Direction};

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("graph-vector.tapir");

    let graph = GraphDatabase::open(&path).unwrap();
    graph.add_node(1, "Person", r#"{"name":"Ada"}"#).unwrap();
    graph
        .add_node(2, "Person", r#"{"name":"Alex Chen"}"#)
        .unwrap();
    graph.add_edge(1, 2, "KNOWS", 1.0, "{}").unwrap();
    assert_eq!(graph.neighbors(1, Direction::Outgoing, None).len(), 1);
    graph.close().unwrap();
    drop(graph);

    let vector = VectorDatabase::open(&path).unwrap();
    let collection = vector.collection("people", 2).unwrap();
    collection
        .insert(7, &[1.0, 0.0], &json!({"name":"Ada"}))
        .unwrap();
    collection
        .insert(8, &[0.0, 1.0], &json!({"name":"Alex Chen"}))
        .unwrap();
    assert!(collection.insert(9, &[f32::NAN, 0.0], &json!({})).is_err());
    assert_eq!(
        collection.search(&[1.0, 0.0], 1).unwrap()[0]
            .get::<i64>("id")
            .unwrap(),
        7
    );
    drop(collection);
    assert!(vector.collection("people", 3).is_err());
    vector.close().unwrap();
    drop(vector);

    let quad = Connection::open(&path).unwrap();
    assert_eq!(quad.graph_neighbors(1, Direction::Outgoing, None).len(), 1);
    assert_eq!(quad.query("SELECT id FROM __vec_people").unwrap().len(), 2);
    quad.close().unwrap();
}

#[cfg(all(feature = "edition-graph", feature = "edition-vector"))]
#[test]
fn graph_and_vector_binaries_run_as_separate_processes() {
    use std::process::Command;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("standalone.tapir");
    let graph = env!("CARGO_BIN_EXE_tapirus-graph");
    let vector = env!("CARGO_BIN_EXE_tapirus-vector");

    let node = Command::new(graph)
        .args([
            path.as_os_str(),
            "node".as_ref(),
            "1".as_ref(),
            "Person".as_ref(),
            r#"{"name":"Ada"}"#.as_ref(),
        ])
        .output()
        .unwrap();
    assert!(
        node.status.success(),
        "{}",
        String::from_utf8_lossy(&node.stderr)
    );

    let second = Command::new(graph)
        .args([
            path.as_os_str(),
            "node".as_ref(),
            "2".as_ref(),
            "Person".as_ref(),
            r#"{"name":"Alex Chen"}"#.as_ref(),
        ])
        .output()
        .unwrap();
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );

    let edge = Command::new(graph)
        .args([
            path.as_os_str(),
            "edge".as_ref(),
            "1".as_ref(),
            "2".as_ref(),
            "KNOWS".as_ref(),
        ])
        .output()
        .unwrap();
    assert!(
        edge.status.success(),
        "{}",
        String::from_utf8_lossy(&edge.stderr)
    );

    let neighbors = Command::new(graph)
        .args([path.as_os_str(), "neighbors".as_ref(), "1".as_ref()])
        .output()
        .unwrap();
    assert!(
        neighbors.status.success(),
        "{}",
        String::from_utf8_lossy(&neighbors.stderr)
    );
    assert!(
        String::from_utf8(neighbors.stdout)
            .unwrap()
            .contains("KNOWS")
    );

    let insert = Command::new(vector)
        .args([
            path.as_os_str(),
            "people".as_ref(),
            "2".as_ref(),
            "insert".as_ref(),
            "1".as_ref(),
            "[1.0,0.0]".as_ref(),
            r#"{"name":"Ada"}"#.as_ref(),
        ])
        .output()
        .unwrap();
    assert!(
        insert.status.success(),
        "{}",
        String::from_utf8_lossy(&insert.stderr)
    );

    let search = Command::new(vector)
        .args([
            path.as_os_str(),
            "people".as_ref(),
            "2".as_ref(),
            "search".as_ref(),
            "[1.0,0.0]".as_ref(),
            "1".as_ref(),
        ])
        .output()
        .unwrap();
    assert!(
        search.status.success(),
        "{}",
        String::from_utf8_lossy(&search.stderr)
    );
    assert!(String::from_utf8(search.stdout).unwrap().contains("Ada"));
}
