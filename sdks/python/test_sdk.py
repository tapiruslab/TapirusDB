import os
import sys
import tapirus

print("Testing TapirusDB Python SDK...")
print("TapirusDB version:", tapirus.version())

# 1. Connect in-memory
conn = tapirus.connect(":memory:")
print("Connected to :memory:")

# 2. Execute SQL
conn.execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, score REAL, vec VECTOR(3));")
print("Table created.")

conn.execute("INSERT INTO users VALUES (1, 'Alice', 95.5, [0.9, 0.1, 0.0]);")
conn.execute("INSERT INTO users VALUES (2, 'Bob', 82.0, [0.1, 0.9, 0.0]);")
conn.execute("INSERT INTO users VALUES (3, 'Charlie', 91.0, [0.8, 0.2, 0.1]);")
print("Inserted 3 rows.")

# 3. Query
rows = conn.query("SELECT id, name, score FROM users ORDER BY score DESC;")
print("Query result:", rows)
assert len(rows) == 3
assert rows[0]['name'] == 'Alice'

# 4. Window function
win_rows = conn.query("SELECT id, name, score, ROW_NUMBER() OVER (ORDER BY score DESC) as rank FROM users;")
print("Window function result:", win_rows)
assert len(win_rows) == 3
assert win_rows[0]['rank'] == 1

# 5. Vector search
vec_matches = conn.vector_search("users", "vec", [0.88, 0.12, 0.0], top_k=2)
print("Vector search result:", vec_matches)
assert len(vec_matches) == 2
assert vec_matches[0]['name'] == 'Alice'

print("PYTHON SDK TEST PASSED WITH FLYING COLORS (GRADE A)!")
