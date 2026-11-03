package tapirus

import (
	"encoding/json"
	"path/filepath"
	"testing"
)

func TestNativeFileReopenAndValues(t *testing.T) {
	file := filepath.Join(t.TempDir(), "consumer.tapir")
	db, err := Open(file)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := db.Execute("CREATE TABLE records (id INTEGER PRIMARY KEY, note TEXT, big INTEGER)"); err != nil {
		t.Fatal(err)
	}
	if _, err := db.Execute("INSERT INTO records VALUES (1, NULL, 9007199254740993)"); err != nil {
		t.Fatal(err)
	}
	if _, err := db.Query("INVALID SQL"); err == nil {
		t.Fatal("Invalid SQL returned success")
	}
	db.Close()
	db.Close()
	if _, err := db.Query("SELECT 1"); err == nil {
		t.Fatal("Closed connection returned success")
	}
	db, err = Open(file)
	if err != nil {
		t.Fatal(err)
	}
	defer db.Close()
	rows, err := db.Query("SELECT * FROM records")
	if err != nil || len(rows) != 1 || rows[0]["note"] != nil || rows[0]["big"] != json.Number("9007199254740993") {
		t.Fatalf("Native reopen lost values: %#v, %v", rows, err)
	}
}
