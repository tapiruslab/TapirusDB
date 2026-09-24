package tapirus

/*
#cgo CFLAGS: -I../../../include -I../../include -I.
#cgo LDFLAGS: -L../../../target/release -L../../target/release -L. -ltapirus
#include <stdlib.h>
#include "tapirus.h"
*/
import "C"
import (
	"encoding/json"
	"errors"
	"unsafe"
)

// DB represents an active connection to a TapirusDB embedded database.
type DB struct {
	conn *C.TapirusConn
}

// Version returns the TapirusDB engine version string.
func Version() string {
	return C.GoString(C.tapirus_version())
}

// Open opens a database file on disk.
func Open(path string) (*DB, error) {
	cPath := C.CString(path)
	defer C.free(unsafe.Pointer(cPath))

	conn := C.tapirus_open(cPath)
	if conn == nil {
		return nil, errors.New("failed to open TapirusDB at path: " + path)
	}
	return &DB{conn: conn}, nil
}

// OpenInMemory opens a transient in-memory database.
func OpenInMemory() (*DB, error) {
	conn := C.tapirus_open_in_memory()
	if conn == nil {
		return nil, errors.New("failed to open in-memory TapirusDB")
	}
	return &DB{conn: conn}, nil
}

// OpenEncrypted opens an encrypted database with ChaCha20-Poly1305.
func OpenEncrypted(path string, passphrase string) (*DB, error) {
	cPath := C.CString(path)
	defer C.free(unsafe.Pointer(cPath))
	cPass := C.CString(passphrase)
	defer C.free(unsafe.Pointer(cPass))

	conn := C.tapirus_open_encrypted(cPath, cPass)
	if conn == nil {
		return nil, errors.New("failed to open encrypted TapirusDB (check passphrase or file)")
	}
	return &DB{conn: conn}, nil
}

// Execute executes a non-query SQL command (CREATE, INSERT, UPDATE, DELETE, BEGIN, COMMIT, etc.).
func (db *DB) Execute(sql string) (int, error) {
	cSql := C.CString(sql)
	defer C.free(unsafe.Pointer(cSql))

	var errStr *C.char
	affected := C.tapirus_execute(db.conn, cSql, &errStr)
	if affected < 0 {
		errMsg := "unknown error"
		if errStr != nil {
			errMsg = C.GoString(errStr)
			C.tapirus_free_string(errStr)
		}
		return 0, errors.New(errMsg)
	}
	return int(affected), nil
}

// Query executes a query and returns rows as structured maps.
func (db *DB) Query(sql string) ([]map[string]interface{}, error) {
	cSql := C.CString(sql)
	defer C.free(unsafe.Pointer(cSql))

	var jsonStr *C.char
	var errStr *C.char

	res := C.tapirus_query_json(db.conn, cSql, &jsonStr, &errStr)
	if res != 0 {
		errMsg := "unknown error"
		if errStr != nil {
			errMsg = C.GoString(errStr)
			C.tapirus_free_string(errStr)
		}
		return nil, errors.New(errMsg)
	}

	if jsonStr == nil {
		return []map[string]interface{}{}, nil
	}

	payload := C.GoString(jsonStr)
	C.tapirus_free_string(jsonStr)

	var rawRows []struct {
		Columns []string      `json:"columns"`
		Values  []interface{} `json:"values"`
	}

	if err := json.Unmarshal([]byte(payload), &rawRows); err != nil {
		return nil, err
	}

	result := make([]map[string]interface{}, 0, len(rawRows))
	for _, r := range rawRows {
		rowMap := make(map[string]interface{})
		for i, col := range r.Columns {
			if i < len(r.Values) {
				val := r.Values[i]
				if valMap, ok := val.(map[string]interface{}); ok {
					for _, v := range valMap {
						val = v
						break
					}
				}
				rowMap[col] = val
			}
		}
		result = append(result, rowMap)
	}

	return result, nil
}

// Close closes the database connection.
func (db *DB) Close() {
	if db.conn != nil {
		C.tapirus_close(db.conn)
		db.conn = nil
	}
}
