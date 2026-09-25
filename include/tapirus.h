/**
 * @file tapirus.h
 * @brief TapirusDB C Application Binary Interface (ABI) Header
 *
 * TapirusDB: The Pure Safe-Rust Embedded Quad-Model AI Database Engine
 * (Relational SQL + Native AI Vector Search + MongoDB-style JSON Documents + Knowledge Graph)
 *
 * Copyright (c) 2026 Ahmad Faiz • Tapirus Tech Lab (TapirusDB.com). All Rights Reserved.
 * Licensed under the Business Source License 1.1 (BSL 1.1).
 */

#ifndef TAPIRUS_H
#define TAPIRUS_H

#include <stdint.h>
#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

/**
 * @brief Opaque handle representing an active database connection.
 */
typedef struct TapirusConn TapirusConn;

/**
 * @brief Open a TapirusDB single-file database at the specified path.
 *
 * If the file does not exist, a new 4,096-byte database will be created.
 *
 * @param path Path to the database file (UTF-8 null-terminated string).
 * @return Pointer to TapirusConn on success, or NULL on failure.
 */
TapirusConn* tapirus_open(const char* path);

/**
 * @brief Open an encrypted TapirusDB single-file database using ChaCha20-Poly1305 AEAD.
 *
 * @param path Path to the database file (UTF-8 null-terminated string).
 * @param passphrase Passphrase used for cryptographic key derivation.
 * @return Pointer to TapirusConn on success, or NULL on failure (e.g. bad passphrase).
 */
TapirusConn* tapirus_open_encrypted(const char* path, const char* passphrase);

/**
 * @brief Open a transient in-memory TapirusDB database.
 *
 * @return Pointer to TapirusConn on success, or NULL on failure.
 */
TapirusConn* tapirus_open_in_memory(void);

/**
 * @brief Close an open database connection and release all associated resources.
 *
 * @param conn Pointer to TapirusConn to close. Safe to call with NULL.
 */
void tapirus_close(TapirusConn* conn);

/**
 * @brief Execute a non-query SQL or multi-model command (CREATE TABLE, INSERT, etc.).
 *
 * @param conn Pointer to active TapirusConn.
 * @param sql Null-terminated SQL statement string.
 * @param err_msg_out Optional pointer to receive a heap-allocated error string.
 *                    Must be freed using tapirus_free_string() if populated.
 * @return Number of affected rows (>= 0) on success, or -1 on failure.
 */
int32_t tapirus_execute(TapirusConn* conn, const char* sql, char** err_msg_out);

/**
 * @brief Execute a SQL query and return rows formatted as a JSON array string.
 *
 * @param conn Pointer to active TapirusConn.
 * @param sql Null-terminated SQL query string (e.g. SELECT ...).
 * @param json_out Pointer to receive a heap-allocated JSON string representing rows.
 *                 Must be freed using tapirus_free_string().
 * @param err_msg_out Optional pointer to receive a heap-allocated error string.
 *                    Must be freed using tapirus_free_string() if populated.
 * @return 0 on success, or -1 on failure.
 */
int32_t tapirus_query_json(TapirusConn* conn, const char* sql, char** json_out, char** err_msg_out);

/**
 * @brief Manually flush the Write-Ahead Log (.tapir-wal) to the main database file.
 *
 * @param conn Pointer to active TapirusConn.
 * @return Number of pages flushed to disk on success, or -1 on failure.
 */
int64_t tapirus_checkpoint(TapirusConn* conn);

/**
 * @brief Free a heap-allocated string returned by tapirus_query_json() or error handlers.
 *
 * @param s Pointer to string to free. Safe to call with NULL.
 */
void tapirus_free_string(char* s);

/**
 * @brief Return the library version string (e.g. "1.0.0").
 *
 * @return Static null-terminated version string. Do NOT free this pointer.
 */
const char* tapirus_version(void);

#ifdef __cplusplus
}
#endif

#endif /* TAPIRUS_H */
