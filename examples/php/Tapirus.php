<?php
/**
 * TapirusDB Native PHP Client via FFI (PHP 7.4+ / PHP 8.x)
 * Zero PECL extensions required.
 */

namespace Tapirus;

class Tapirus {
    private static ?\FFI $ffi = null;
    private $handle;

    public static function ffi(): \FFI {
        if (self::$ffi === null) {
            $cdef = "
                typedef struct TapirusConn TapirusConn;
                const char* tapirus_version(void);
                TapirusConn* tapirus_open(const char* path);
                TapirusConn* tapirus_open_in_memory(void);
                TapirusConn* tapirus_open_encrypted(const char* path, const char* passphrase);
                void tapirus_close(TapirusConn* conn);
                int32_t tapirus_execute(TapirusConn* conn, const char* sql, char** err_msg_out);
                int32_t tapirus_query_json(TapirusConn* conn, const char* sql, char** json_out, char** err_msg_out);
                void tapirus_free_string(char* s);
                int64_t tapirus_checkpoint(TapirusConn* conn);
            ";

            $home = getenv('HOME') ?: '';
            $candidates = [
                getenv('TAPIRUS_LIB') ?: '',
                $home . '/.tapirusdb-target/release/libtapirus.so',
                __DIR__ . '/../../target/release/libtapirus.so',
                __DIR__ . '/../../target/release/tapirus.dll',
                'libtapirus.so',
            ];

            $libPath = 'libtapirus.so';
            foreach ($candidates as $c) {
                if ($c && file_exists($c)) {
                    $libPath = $c;
                    break;
                }
            }

            self::$ffi = \FFI::cdef($cdef, $libPath);
        }
        return self::$ffi;
    }

    public function __construct(?string $path = null, ?string $passphrase = null) {
        $ffi = self::ffi();
        if ($path === null || $path === ':memory:') {
            $this->handle = $ffi->tapirus_open_in_memory();
        } elseif ($passphrase !== null) {
            $this->handle = $ffi->tapirus_open_encrypted($path, $passphrase);
        } else {
            $this->handle = $ffi->tapirus_open($path);
        }

        if ($this->handle === null) {
            throw new \RuntimeException("Failed to open TapirusDB database at: " . ($path ?? ':memory:'));
        }
    }

    public static function version(): string {
        return self::ffi()->tapirus_version();
    }

    public function execute(string $sql): int {
        $ffi = self::ffi();
        $errPtr = $ffi->new('char*');
        $affected = $ffi->tapirus_execute($this->handle, $sql, \FFI::addr($errPtr));
        if ($affected < 0) {
            $errMsg = $errPtr !== null ? \FFI::string($errPtr) : "Unknown error";
            if ($errPtr !== null) {
                $ffi->tapirus_free_string($errPtr);
            }
            throw new \RuntimeException("Execute failed: $errMsg");
        }
        return $affected;
    }

    public function query(string $sql): array {
        $ffi = self::ffi();
        $jsonPtr = $ffi->new('char*');
        $errPtr = $ffi->new('char*');
        $res = $ffi->tapirus_query_json($this->handle, $sql, \FFI::addr($jsonPtr), \FFI::addr($errPtr));
        if ($res !== 0) {
            $errMsg = $errPtr !== null ? \FFI::string($errPtr) : "Unknown error";
            if ($errPtr !== null) {
                $ffi->tapirus_free_string($errPtr);
            }
            throw new \RuntimeException("Query failed: $errMsg");
        }

        if ($jsonPtr === null) {
            return [];
        }

        $jsonStr = \FFI::string($jsonPtr);
        $ffi->tapirus_free_string($jsonPtr);

        $rawRows = json_decode($jsonStr, true) ?? [];
        $rows = [];
        foreach ($rawRows as $r) {
            $cols = $r['columns'] ?? [];
            $vals = $r['values'] ?? [];
            $row = [];
            foreach ($cols as $idx => $col) {
                $v = $vals[$idx] ?? null;
                if (is_array($v)) {
                    $v = reset($v);
                }
                $row[$col] = $v;
            }
            $rows[] = $row;
        }
        return $rows;
    }

    public function close(): void {
        if ($this->handle !== null) {
            self::ffi()->tapirus_close($this->handle);
            $this->handle = null;
        }
    }

    public function __destruct() {
        $this->close();
    }
}
