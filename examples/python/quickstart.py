import sys
sys.path.insert(0, "python")
from tapirus import Tapirus

def main():
    print(f"Testing TapirusDB Python bindings (v{Tapirus.version()})...")

    # In-memory database
    with Tapirus() as db:
        db.execute("CREATE TABLE heroes (id INTEGER PRIMARY KEY, name TEXT, power REAL);")
        db.execute("INSERT INTO heroes (id, name, power) VALUES (1, 'Iron Man', 99.5);")
        db.execute("INSERT INTO heroes (id, name, power) VALUES (2, 'Thor', 100.0);")

        # Test UPDATE & DELETE
        db.execute("UPDATE heroes SET power = 105.0 WHERE id = 1;")
        db.execute("DELETE FROM heroes WHERE id = 2;")

        rows = db.query("SELECT id, name, power FROM heroes;")
        print("Query Results:", rows)
        assert len(rows) == 1
        assert rows[0]["name"] == "Iron Man"
        assert rows[0]["power"] == 105.0

    print("ALL PYTHON TESTS PASSED SUCCESSFULLY!")

if __name__ == "__main__":
    main()
