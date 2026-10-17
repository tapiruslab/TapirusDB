use tapirus::{Connection, Value};

fn sqlite_value(value: rusqlite::types::ValueRef<'_>) -> Value {
    use rusqlite::types::ValueRef;
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(value) => Value::Integer(value),
        ValueRef::Real(value) => Value::Real(value),
        ValueRef::Text(value) => Value::Text(String::from_utf8(value.to_vec()).unwrap()),
        ValueRef::Blob(value) => Value::Blob(value.to_vec()),
    }
}

#[test]
fn function_names_work_as_identifiers_and_aggregate_aliases() {
    let tapir = Connection::open_in_memory().unwrap();
    let sqlite = rusqlite::Connection::open_in_memory().unwrap();
    for sql in [
        "CREATE TABLE metrics (id INTEGER PRIMARY KEY, count INTEGER, sum INTEGER, rank INTEGER);",
        "INSERT INTO metrics VALUES (1, 4, 7, 3), (2, 8, 9, 1), (3, 12, 11, 2);",
    ] {
        tapir.execute(sql).unwrap();
        sqlite.execute(sql, []).unwrap();
    }
    for sql in [
        "SELECT count, sum, rank FROM metrics WHERE count >= 8 ORDER BY rank;",
        "SELECT COUNT(*) AS count, SUM(count) AS sum, AVG(count) AS avg, MIN(count) AS min, MAX(count) AS max FROM metrics;",
        "SELECT count AS sum, sum AS count FROM metrics ORDER BY id;",
        "SELECT count, SUM(sum) AS sum FROM metrics GROUP BY count ORDER BY count;",
        "WITH filtered AS (SELECT count FROM metrics) SELECT COUNT(*) AS count FROM filtered;",
    ] {
        let rows = tapir.query(sql).unwrap();
        let column_names = rows[0].columns().to_vec();
        let actual: Vec<Vec<Value>> = rows.into_iter().map(|row| row.values().to_vec()).collect();
        let mut statement = sqlite.prepare(sql).unwrap();
        assert_eq!(
            column_names,
            statement.column_names(),
            "Column aliases: {sql}"
        );
        let width = statement.column_count();
        let expected = statement
            .query_map([], |row| {
                Ok((0..width)
                    .map(|i| sqlite_value(row.get_ref(i).unwrap()))
                    .collect::<Vec<_>>())
            })
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(actual, expected, "SQL compatibility: {sql}");
    }
    let bound = tapir
        .query_with_params(
            "SELECT COUNT(*) AS count FROM metrics WHERE count >= ?;",
            &[Value::Integer(8)],
        )
        .unwrap();
    assert_eq!(bound[0].get::<i64>("count").unwrap(), 2);
}
