/*
** 2026-09-22
**
** The author disclaims copyright to this source code.  In place of
** a legal notice, here is a blessing:
**
**    May you do good and not evil.
**    May you find forgiveness for yourself and forgive others.
**    May you share freely, never taking more than you give.
**
*************************************************************************
** This header file defines the C interface for TapirusDB's SQLite C ABI
** compatibility layer.
*/
#ifndef SQLITE3_H
#define SQLITE3_H

#include <stdarg.h>

#ifdef __cplusplus
extern "C" {
#endif

#ifndef SQLITE_API
# define SQLITE_API
#endif

#define SQLITE_OK           0   /* Successful result */
#define SQLITE_ERROR        1   /* Generic error */
#define SQLITE_INTERNAL     2   /* Internal logic error in SQLite */
#define SQLITE_PERM         3   /* Access permission denied */
#define SQLITE_ABORT        4   /* Callback routine requested an abort */
#define SQLITE_BUSY         5   /* The database file is locked */
#define SQLITE_LOCKED       6   /* A table in the database is locked */
#define SQLITE_NOMEM        7   /* A malloc() failed */
#define SQLITE_ROW        100   /* sqlite3_step() has another row ready */
#define SQLITE_DONE       101   /* sqlite3_step() has finished executing */

#define SQLITE_INTEGER  1
#define SQLITE_FLOAT    2
#define SQLITE_TEXT     3
#define SQLITE_BLOB     4
#define SQLITE_NULL     5

typedef struct sqlite3 sqlite3;
typedef struct sqlite3_stmt sqlite3_stmt;
typedef long long sqlite3_int64;
typedef unsigned long long sqlite3_uint64;

SQLITE_API int sqlite3_open(const char *filename, sqlite3 **ppDb);
SQLITE_API int sqlite3_open_v2(const char *filename, sqlite3 **ppDb, int flags, const char *zVfs);
SQLITE_API int sqlite3_close(sqlite3 *db);
SQLITE_API int sqlite3_close_v2(sqlite3 *db);

SQLITE_API int sqlite3_prepare_v2(
  sqlite3 *db,
  const char *zSql,
  int nByte,
  sqlite3_stmt **ppStmt,
  const char **pzTail
);

SQLITE_API int sqlite3_step(sqlite3_stmt *pStmt);
SQLITE_API int sqlite3_finalize(sqlite3_stmt *pStmt);
SQLITE_API int sqlite3_reset(sqlite3_stmt *pStmt);

SQLITE_API int sqlite3_column_count(sqlite3_stmt *pStmt);
SQLITE_API const char *sqlite3_column_name(sqlite3_stmt *pStmt, int N);
SQLITE_API int sqlite3_column_type(sqlite3_stmt *pStmt, int N);
SQLITE_API const unsigned char *sqlite3_column_text(sqlite3_stmt *pStmt, int N);
SQLITE_API int sqlite3_column_int(sqlite3_stmt *pStmt, int N);
SQLITE_API sqlite3_int64 sqlite3_column_int64(sqlite3_stmt *pStmt, int N);
SQLITE_API double sqlite3_column_double(sqlite3_stmt *pStmt, int N);
SQLITE_API int sqlite3_column_bytes(sqlite3_stmt *pStmt, int N);

SQLITE_API const char *sqlite3_errmsg(sqlite3 *db);
SQLITE_API int sqlite3_errcode(sqlite3 *db);
SQLITE_API int sqlite3_changes(sqlite3 *db);
SQLITE_API const char *sqlite3_libversion(void);
SQLITE_API int sqlite3_libversion_number(void);

#ifdef __cplusplus
}
#endif

#endif /* SQLITE3_H */
