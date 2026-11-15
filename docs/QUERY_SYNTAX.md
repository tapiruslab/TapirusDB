# TapirusDB Complete Query Syntax Reference

> **Applies to:** TapirusDB `v1.1.0` (and `olap` / `bigdata` pushdowns on `main`) across Rust (`Connection` / `SessionDatabase`), Python, Node.js, Browser WASM (`@tapirus/wasm` & `@tapirus/wasm-lite`), CLI (`tapirus`), and HTTP/TLS (`tapirus serve`).

This guide is designed so **any developer**—whether coming from SQLite/Postgres or building vector, graph, or Parquet analytics—can find the exact working query syntax in seconds.

---

## 1. Quick Cheat Sheet (At a Glance)

| Task | Exact Syntax |
| :--- | :--- |
| Create SQL + Vector table | `CREATE TABLE docs (id INTEGER PRIMARY KEY, title TEXT NOT NULL, emb VECTOR(384));` |
| Insert row(s) | `INSERT INTO docs (id, title) VALUES (1, 'Hello'), (2, 'World');` |
| Upsert on conflict | `INSERT INTO docs (id, title) VALUES (1, 'New') ON CONFLICT DO UPDATE SET title = EXCLUDED.title;` |
| Query + filter + sort | `SELECT id, title FROM docs WHERE id >= 1 ORDER BY id DESC LIMIT 10 OFFSET 0;` |
| Recursive hierarchy (CTE) | `WITH RECURSIVE tree AS (SELECT ... UNION ALL SELECT ... JOIN tree ...) SELECT * FROM tree;` |
| Partial index | `CREATE INDEX idx_active ON docs (title) WHERE title IS NOT NULL;` |
| Full-text search (`FTS5`) | `CREATE VIRTUAL TABLE docs_fts USING fts5(title, body);` then `WHERE docs_fts MATCH 'query'` |
| Vector nearest neighbors | `SELECT id, title FROM docs VECTOR NEAR emb = [0.1, 0.2, 0.3] TOP 5;` |
| Graph node & edge | `GRAPH INSERT NODE 1 LABEL 'Person' PROPERTIES '{"name":"Ali"}';` |
| Graph pattern query | `GRAPH MATCH (a:Person)-[r:KNOWS]->(b:Person) WHERE a.id = 1 RETURN a, r, b;` |
| Query Parquet / Excel / CSV | `SELECT region, COUNT(*), MEDIAN(ms) FROM read_parquet('lake/**/*.parquet') GROUP BY region;` |
| Export query to Parquet | `COPY (SELECT * FROM docs) TO 'out/docs' (FORMAT PARQUET, PARTITION_BY year);` |
| Local AI classification | `SELECT id, TAP_CLASSIFY(title, 'billing, support, sales') AS tag FROM docs;` |
| Inspect schema | `PRAGMA table_info('docs');` |

---

## 2. Data Types & Table DDL

### Column Data Types
- `INTEGER` — 64-bit signed integer (`i64`)
- `REAL` — 64-bit IEEE float (`f64`)
- `TEXT` — UTF-8 string (also stores JSON objects/arrays queried via `->>`)
- `BLOB` — raw binary bytes
- `VECTOR(D)` — fixed-dimension dense `f32` vector (e.g., `VECTOR(384)`, `VECTOR(1536)`)

### `CREATE TABLE`, Constraints & Views
```sql
CREATE TABLE IF NOT EXISTS employees (
    id INTEGER PRIMARY KEY,
    dept_id INTEGER NOT NULL REFERENCES departments(id) ON DELETE CASCADE ON UPDATE CASCADE,
    email TEXT UNIQUE,
    age INTEGER NOT NULL CHECK (age >= 18),
    role TEXT DEFAULT 'engineer',
    profile TEXT,
    embedding VECTOR(4)
);

-- Time-travel / temporal table (system versioning)
CREATE TABLE audit_log (id INTEGER PRIMARY KEY, msg TEXT) WITH SYSTEM VERSIONING;

-- Full-text search virtual table (FTS5)
CREATE VIRTUAL TABLE articles_fts USING fts5(title, body);

-- Views
CREATE VIEW IF NOT EXISTS senior_staff AS
SELECT id, email, role FROM employees WHERE age >= 30;

DROP VIEW IF EXISTS senior_staff;
```

### `ALTER TABLE` & `DROP TABLE`
```sql
ALTER TABLE employees ADD COLUMN phone TEXT;
ALTER TABLE employees RENAME COLUMN phone TO mobile;
ALTER TABLE employees RENAME TO staff;
ALTER TABLE employees DROP COLUMN mobile;
DROP TABLE IF EXISTS staff;
```

---

## 3. Writing Data (`INSERT`, `UPDATE`, `DELETE`) & Transactions

### Single-Row, Multi-Row Batch & Conflict Resolution
```sql
-- Standard multi-row SQL insert (fast batch path)
INSERT INTO employees (id, dept_id, age, role)
VALUES (1, 10, 29, 'backend'),
       (2, 10, 34, 'ml');

-- SQLite-style conflict handling
INSERT OR REPLACE INTO employees (id, dept_id, age) VALUES (1, 10, 30);
INSERT OR IGNORE  INTO employees (id, dept_id, age) VALUES (1, 10, 30);

-- PostgreSQL / SQLite ON CONFLICT upsert
INSERT INTO employees (id, dept_id, age, role)
VALUES (1, 10, 31, 'lead')
ON CONFLICT DO UPDATE SET age = EXCLUDED.age, role = EXCLUDED.role
RETURNING id, role;

INSERT INTO employees (id, dept_id, age)
VALUES (1, 10, 31)
ON CONFLICT DO NOTHING;
```

> **Performance tip:** For high-volume Rust ingestion (>1,000 rows), `Connection::insert_rows_batch()` bypasses per-row SQL parsing (`380k–471k rows/sec` in release builds), while wrapping SQL statements in `BEGIN; ... COMMIT;` uses the batched commit fast path.

### `UPDATE` & `DELETE`
```sql
UPDATE employees SET role = 'principal', age = age + 1 WHERE id = 1 RETURNING id, role;
DELETE FROM employees WHERE age < 18 RETURNING id;
```

### Transactions & Savepoints
```sql
BEGIN;
  INSERT INTO departments (id, name) VALUES (20, 'Research');
  SAVEPOINT sp1;
  INSERT INTO departments (id, name) VALUES (21, 'Temp');
  ROLLBACK TO SAVEPOINT sp1;
  RELEASE SAVEPOINT sp1;
COMMIT;
-- Or ROLLBACK;
```

---

## 4. Querying Data (`SELECT`, Joins, CTEs, Set Ops, Windows)

### Core `SELECT` Clause
```sql
SELECT DISTINCT dept_id,
       role,
       age + 1 AS next_age,
       email || ' (' || role || ')' AS badge
FROM employees
WHERE age BETWEEN 25 AND 55
  AND role IN ('backend', 'lead')
  AND email LIKE '%@tapirusdb.com'
  AND dept_id IS NOT NULL
GROUP BY dept_id, role
HAVING COUNT(*) >= 1
ORDER BY dept_id ASC, next_age DESC
LIMIT 50 OFFSET 0;
```

**Supported `WHERE` predicates:** `=`, `!=` / `<>`, `<`, `<=`, `>`, `>=`, `AND`, `OR`, `IN (...)`, `IN (SELECT ...)`, `EXISTS (SELECT ...)`, `LIKE`, `IS NULL`, `IS NOT NULL`, `BETWEEN ... AND ...`.

**Supported infix expressions in projections:** `+`, `-`, `*`, `/`, `%`, string concatenation `||`, parentheses `(...)`.

### Joins & Derived Subqueries
```sql
SELECT e.id, e.role, d.name AS department
FROM employees e
INNER JOIN departments d ON e.dept_id = d.id;

-- Also supported:
-- LEFT [OUTER] JOIN ... ON ...
-- RIGHT [OUTER] JOIN ... ON ...
-- FULL [OUTER] JOIN ... ON ...

-- Derived subquery in FROM
SELECT sub.dept_id, sub.total
FROM (SELECT dept_id, COUNT(*) AS total FROM employees GROUP BY dept_id) AS sub
WHERE sub.total > 5;
```

### CTEs (`WITH [RECURSIVE]`) & Set Operations (`UNION [ALL]`)
```sql
WITH RECURSIVE org_tree AS (
    SELECT id, manager_id, name, 0 AS depth FROM org WHERE manager_id IS NULL
    UNION ALL
    SELECT o.id, o.manager_id, o.name, t.depth + 1
    FROM org o
    JOIN org_tree t ON o.manager_id = t.id
)
SELECT id, name, depth FROM org_tree ORDER BY depth ASC;

-- Set combination
SELECT name FROM active_users
UNION        -- deduplicates
SELECT name FROM archived_users;

SELECT name FROM active_users
UNION ALL    -- keeps all rows
SELECT name FROM archived_users;
```

### Window Functions
Supported window functions with `OVER ([PARTITION BY col] [ORDER BY col [ASC|DESC]])`:
- Ranking: `ROW_NUMBER()`, `RANK()`, `DENSE_RANK()`, `NTILE(n)`
- Offset: `LAG(col, [offset])`, `LEAD(col, [offset])`
- Aggregate windows: `COUNT(*)`, `COUNT(col)`, `SUM(col)`, `AVG(col)`, `MIN(col)`, `MAX(col)`

```sql
SELECT id, dept_id, salary,
       ROW_NUMBER() OVER (PARTITION BY dept_id ORDER BY salary DESC) AS rn,
       DENSE_RANK() OVER (ORDER BY salary DESC) AS drank,
       LAG(salary, 1) OVER (PARTITION BY dept_id ORDER BY id ASC) AS prev_salary
FROM payroll;
```

---

## 5. Indexes, Full-Text Search (FTS5), Triggers & Time Travel

### Standard, Composite, Unique & Partial Indexes
```sql
CREATE INDEX IF NOT EXISTS idx_emp_dept_role ON employees (dept_id, role);
CREATE UNIQUE INDEX idx_emp_email ON employees (email);

-- Partial index (v1.1.0+): only indexes rows matching the WHERE predicate
CREATE INDEX idx_high_latency ON telemetry (endpoint, latency_ms)
WHERE status_code >= 500;

DROP INDEX IF EXISTS idx_high_latency;
```

### FTS5 Full-Text Search
```sql
CREATE VIRTUAL TABLE docs_fts USING fts5(title, body);
INSERT INTO docs_fts (title, body) VALUES ('WAL Guide', 'Atomic crash recovery and snapshots');
SELECT title, body FROM docs_fts WHERE docs_fts MATCH 'recovery';
```

### Triggers & `RAISE`
```sql
CREATE TRIGGER IF NOT EXISTS prevent_negative_balance
BEFORE UPDATE ON accounts
FOR EACH ROW
WHEN (NEW.balance < 0)
BEGIN
    SELECT RAISE(ABORT, 'balance cannot be negative');
END;

DROP TRIGGER IF EXISTS prevent_negative_balance;
```
Supported timings: `BEFORE` / `AFTER` on `INSERT` / `UPDATE` / `DELETE`; `RAISE` actions: `ABORT`, `FAIL`, `ROLLBACK`, `IGNORE`.

### Time-Travel Query
```sql
PRAGMA time_travel = on;
SELECT * FROM audit_log AS OF TIMESTAMP 1731600000;
```

---

## 6. JSON Documents & Vector AI Search

### JSON Extraction inside SQL
```sql
SELECT id, profile->>'city' AS city
FROM employees
WHERE profile->>'tier' = 'enterprise';
```

### Vector Nearest-Neighbor Query (`VECTOR NEAR ... TOP k`)
```sql
SELECT id, title
FROM docs
VECTOR NEAR embedding = [0.12, -0.44, 0.89, 0.03] TOP 5
WHERE category = 'engineering';
```

### Scalar Vector Distance Functions
```sql
SELECT id,
       VECTOR_COSINE(embedding, [0.12, -0.44, 0.89, 0.03]) AS sim,
       VECTOR_L2(embedding, [0.12, -0.44, 0.89, 0.03]) AS l2,
       VECTOR_DISTANCE(embedding, [0.12, -0.44, 0.89, 0.03]) AS dist
FROM docs
ORDER BY sim DESC
LIMIT 5;
```

---

## 7. Graph & GraphRAG Query Syntax

### Insert Nodes & Weighted Directed Edges
```sql
GRAPH INSERT NODE 1 LABEL 'Service' PROPERTIES '{"name":"auth-api","tier":1}';
GRAPH INSERT NODE 2 LABEL 'Database' PROPERTIES '{"name":"users-db"}';
GRAPH INSERT EDGE 1 -> 2 LABEL 'DEPENDS_ON' WEIGHT 0.95 PROPERTIES '{"critical":true}';
```

### Pattern Matching (`GRAPH MATCH`)
```sql
GRAPH MATCH (s:Service)-[r:DEPENDS_ON]->(d:Database)
WHERE s.tier = 1
RETURN s, r, d;
```

### Multi-Hop Traversal & Shortest Path
```sql
GRAPH TRAVERSE FROM 1 OUTGOING LABEL 'DEPENDS_ON' MAX_DEPTH 3 WHERE target.tier = 2;
-- Direction: OUTGOING | INCOMING | BOTH

GRAPH SHORTEST_PATH FROM 1 TO 42;
```

### Built-In Graph Algorithms in SQL
```sql
GRAPH ALGORITHM PAGERANK damping=0.85 iterations=20 tolerance=0.0001;
GRAPH ALGORITHM CONNECTED_COMPONENTS;
GRAPH ALGORITHM BETWEENNESS;
GRAPH ALGORITHM LOUVAIN;
```

---

## 8. Big Data, Lakehouse & Vectorized Analytics

### Query External Files Directly in SQL (`ExternalScan`)
| Function | Formats Read | Example |
| :--- | :--- | :--- |
| `read_parquet('path')` | Apache Parquet (+ Hive globs `a/**/*.parquet`) | `SELECT * FROM read_parquet('lake/**/*.parquet') LIMIT 20;` |
| `read_excel('path', ['sheet'])` / `read_xlsx(...)` | `.xlsx`, `.xls`, `.xlsb`, `.ods` | `SELECT * FROM read_excel('sales.xlsx', 'Q4');` |
| `read_csv('path')` / `read_tsv('path')` | CSV, `.csv.gz`, TSV | `SELECT COUNT(*) FROM read_csv('logs/*.csv.gz');` |
| `read_jsonl('path')` / `read_ndjson` / `read_json` | JSON Lines / JSON arrays | `SELECT * FROM read_jsonl('events.jsonl');` |
| `read_sqlite('path', 'table')` | Pure-Rust SQLite3 `.db` / `.sqlite` | `SELECT * FROM read_sqlite('legacy.db', 'orders');` |
| `read_pdf('path')` | Adobe `.pdf` pages | `SELECT * FROM read_pdf('manual.pdf');` |
| `read_text('path')` / `read_md('path')` | Plain text, logs, Markdown | `SELECT * FROM read_md('docs/*.md');` |

### Statistical & Vectorized Aggregate Functions
```sql
SELECT region,
       COUNT(*),
       COUNT(DISTINCT user_id) AS unique_users,
       SUM(amount),
       AVG(latency_ms),
       MIN(latency_ms),
       MAX(latency_ms),
       STDDEV(latency_ms) AS stdev_samp,
       STDDEV_POP(latency_ms) AS stdev_pop,
       VARIANCE(latency_ms) AS var_samp,
       MEDIAN(latency_ms) AS p50,
       PERCENTILE(latency_ms, 0.95) AS p95
FROM read_parquet('lake/**/*.parquet')
WHERE region = 'apac' AND latency_ms > 100
GROUP BY region
HAVING COUNT(*) > 10
ORDER BY p95 DESC
LIMIT 20;
```

> **Modular OLAP engine (`olap` + `bigdata` features on `main`):** Automatically applies Hive partition directory pruning, Parquet row-group min/max zone-map pruning, column projection pushdown (`ProjectionMask`), metadata-only `COUNT(*)`, and multi-file parallel execution. In Rust, inspect pushdown telemetry directly via `let (rows, stats) = db.query_olap(sql)?;`.

### Bi-Directional `COPY FROM` / `COPY TO`
```sql
-- Bulk load from Parquet, CSV, TSV, JSONL, SQLite, SQL dump, PDF, or Text
COPY events FROM 'incoming/events.parquet' (FORMAT PARQUET);
COPY legacy_orders FROM 'app.sqlite' (FORMAT SQLITE, TABLE 'orders');

-- Export table or SELECT result to Parquet with optional Hive partitioning
COPY (SELECT * FROM events WHERE year = 2026)
  TO 'exports/events_lake' (FORMAT PARQUET, PARTITION_BY region);
```

---

## 9. TAP Decision Functions in SQL

```sql
-- Classify text into candidate labels
SELECT id, TAP_CLASSIFY(body, 'billing, technical, sales') AS label FROM tickets;

-- Grounded classification using an existing context column
SELECT id, TAP_CLASSIFY_GROUNDED(body, context_col, 'refund, bug, question') FROM tickets;

-- Verify hypothesis (returns 1 or 0)
SELECT id, TAP_VERIFY(body, 'payment failed') AS is_payment_error FROM tickets;
SELECT id, TAP_VERIFY_GROUNDED(body, context_col, 'approved') FROM tickets;

-- Continuous relevance score & routing
SELECT id,
       TAP_SCORE(body, 'urgent outage') AS urgency,
       TAP_ROUTE(body, 'escalate_l2, normal_queue') AS route
FROM tickets;
```

---

## 10. Built-In Scalar Functions, Pragmas & Maintenance

### Scalar Functions
`UPPER(s)`, `LOWER(s)`, `LENGTH(x)` / `LEN(x)`, `ABS(n)`, `ROUND(n, [decimals])`, `COALESCE(v1, v2, ...)`, `CONCAT(s1, s2, ...)`, `SUBSTR(s, start, [len])`, `TRIM(s)`, `REVERSE(s)`.

### Pragmas, Diagnostics & Maintenance
```sql
PRAGMA table_info('employees');     -- cid, name, type, notnull, dflt_value, pk
PRAGMA foreign_keys = on;           -- 1/true/on or 0/false/off
PRAGMA time_travel = on;            -- temporal versioning default

EXPLAIN SELECT * FROM employees WHERE dept_id = 10;
EXPLAIN QUERY PLAN SELECT * FROM employees WHERE dept_id = 10;

ANALYZE employees;                  -- collect table/index statistics (or omit table for all)
VACUUM;                             -- compact database pages
VACUUM INTO 'backup_clean.tapir';   -- write compacted copy
```
