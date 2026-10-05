# 🏛️ TapirusDB Modular Architecture & Extension Blueprint
> **Dokumen Pelan Seni Bina & Peta Hala Tuju Modulariti**  
> *Seni Bina Variasi Pangkalan Data: Penyesuai Protokol PostgreSQL & MongoDB, Seni Bina Mikro-Kernel, dan Sistem Sambungan WebAssembly (WASM).*

---

## 📌 Ringkasan Eksekutif (*Executive Summary*)

Untuk membolehkan TapirusDB berkembang menjadi platform pangkalan data gred perusahaan yang fleksibel (menyaingi PostgreSQL dalam dunia relasi dan MongoDB dalam dunia dokumen tanpa mengorbankan asas teras 100% Safe Rust), kita menetapkan 3 teknik seni bina moden:

1. **Wire Protocol Emulation (`tapirus-pgwire` & `tapirus-mongowire`)**: Membolehkan alat sedia ada (Prisma ORM, DBeaver, Mongoose, Compass) menyambung terus tanpa mengubah kod pelanggan.
2. **Micro-Kernel Workspace (`crates/`)**: Memisahkan enjin storan teras daripada parser dan perkhidmatan luar untuk menghasilkan edisi khas (*Lite, AI, NoSQL, Enterprise*).
3. **Sandboxed WebAssembly (WASM) Extensions**: Membenarkan sambungan fungsi pihak ketiga tanpa risiko *memory corruption* atau *segfault*.

---

## 🌐 Teknik 1: Penyesuaian Protokol Dawai (*Wire Protocol Adapters*)

Daripada memaksa pengguna memasang pemacu (*driver*) baharu, TapirusDB bertindak sebagai pelayan yang memahami protokol PostgreSQL v3 dan MongoDB BSON secara natif.

```text
┌────────────────────────────────────────────────────────────────────────┐
│                        Ekosistem Aplikasi Klien                        │
├───────────────────────────┬────────────────────────────────────────────┤
│   Pelanggan Relasi / SQL  │         Pelanggan NoSQL / Dokumen          │
│  (Prisma, DBeaver, psql)  │       (Mongoose, MongoDB Compass)          │
└─────────────┬─────────────┴─────────────────────┬──────────────────────┘
              │ (Port 5432)                       │ (Port 27017)
              ▼                                   ▼
┌───────────────────────────┐       ┌────────────────────────────────────┐
│      tapirus-pgwire       │       │         tapirus-mongowire          │
│ (PostgreSQL Wire Adapter) │       │       (MongoDB Wire Adapter)       │
└─────────────┬─────────────┘       └─────────────┬──────────────────────┘
              │                                   │
              └─────────────────┬─────────────────┘
                                │
                                ▼
┌────────────────────────────────────────────────────────────────────────┐
│                    TapirusDB Core Engine (Safe Rust)                   │
│        Slotted-Page B+Tree • HNSW Vector • openCypher • JSON           │
│                       Fail Tunggal: `storage.tapir`                    │
└────────────────────────────────────────────────────────────────────────┘
```

### 1.1 Modul PostgreSQL (`tapirus-pgwire`)
* **Port Lalai:** `5432`
* **Pustaka Asas:** Rust crate `pgwire` (atau `postgres-protocol`).
* **Kelebihan:**
  * Pengguna Node.js / Python / Go boleh menyambung menggunakan pemacu standard:
    ```typescript
    import { PrismaClient } from '@prisma/client';
    const prisma = new PrismaClient({
      datasourceUrl: "postgresql://postgres:secret@localhost:5432/production"
    });
    ```
  * Alat GUI visual popular (DBeaver, TablePlus, Navicat) boleh terus membuka jadual TapirusDB.

### 1.2 Modul MongoDB (`tapirus-mongowire`)
* **Port Lalai:** `27017`
* **Protokol:** Menyahkod mesej BSON OP_MSG (`find`, `insert`, `update`, `delete`).
* **Pemetan Dalaman:**
  * Koleksi Mongo dipetakan secara automatik ke dalam enjin storan JSON Document TapirusDB.
  * Indeks B-Tree digunakan untuk carian `_id` dan kolum indeks skema dinamik.

---

## 🧩 Teknik 2: Seni Bina Mikro-Kernel & Modular Crates

Mengelakkan penggelembungan saiz (*bloat*) dengan membahagikan kod ke dalam Cargo Workspace:

```text
TapirusDB/
  ├── Cargo.toml (Workspace Root)
  ├── crates/
  │     ├── tapirus-core/        <-- Enjin Storan Teras (Slotted B+Tree, WAL, XChaCha20, CRC32)
  │     ├── tapirus-sql/         <-- Parser & Perancang SQL-92, Window Functions, Join Engine
  │     ├── tapirus-vector/      <-- HNSW, IVF, RaBitQ SIMD 16-Lane, Cosine Distance
  │     ├── tapirus-graph/       <-- openCypher Parser, CSR Topology, GraphRAG
  │     ├── tapirus-doc/         <-- NoSQL Document Collections, BSON/JSON Path Indexer
  │     ├── tapirus-pgwire/      <-- Penyesuai Pelayan PostgreSQL (Port 5432)
  │     ├── tapirus-mongowire/   <-- Penyesuai Pelayan MongoDB (Port 27017)
  │     └── tapirus-wasm-ext/    <-- Enjin Pelaksana Sandbox Plugin WebAssembly
```

### Variasi Edisi Berdaulat (*Sovereign Edition Profiles*):

| Edisi Rasmi | Ciri Cargo (*Feature Flags*) | Sasaran Penggunaan | Jejak Saiz |
| :--- | :--- | :--- | :--- |
| 🪶 **Tapirus Lite** | `--features edition-lite` (`sql`) | Mikropemproses, IoT, Bare-metal ESP32/STM32 | **< 500 KB** |
| 📄 **Tapirus Document** | `--features edition-document` (`documents`, `sql`) | Pengganti storan dokumen NoSQL JSON/BSON | **~ 1.2 MB** |
| 🧠 **Tapirus AI Brain** | `--features edition-ai-brain` (`vectors`, `graphs`, `tap`) | Robotik, Dron ADAS, Memori Ejen AI Kognitif | **~ 2.5 MB** |
| 👑 **Tapirus Quad** *(Default)* | `--features quad` (*Semua 4 Model Bersatu*) | Pembangun AI & Aplikasi Moden Serbaguna | **~ 3.8 MB** |
| 🏢 **Tapirus Enterprise** | `--features edition-enterprise` (`quad`, `server`) | Pelayan VPS, Awan, Integrasi Prisma/ORM | **~ 6.0 MB** |

```bash
# Contoh kompilasi mengikut profil edisi:
cargo build --release --no-default-features --features edition-lite
cargo build --release --no-default-features --features edition-document
cargo build --release --no-default-features --features edition-ai-brain
cargo build --release --features quad # (Lalai / Flagship)
```

---

## 🔌 Teknik 3: Sistem Plugin Sandbox WebAssembly (WASM Extensions)

Untuk mengekalkan prinsip **100% Safe Rust**, TapirusDB melarang pemuatan binari C dinamik (`.so` / `.dll`) yang berisiko merosakkan memori pelayan. Sebagai gantinya, sistem sambungan moden berasaskan WebAssembly digunakan:

```sql
-- Memuatkan sambungan selamat pihak ketiga ke dalam runtime
LOAD EXTENSION 'extensions/geospatial.wasm';

-- Menggunakan fungsi yang didaftarkan oleh modul WASM
SELECT id, name, ST_Distance(location, POINT(3.1390, 101.6869)) AS distance_km
FROM stores
WHERE distance_km < 10.0;
```

### Ciri Keselamatan Plugin WASM:
1. **Kotak Pasir Penuh (*Full Memory Isolation*):** Modul plugin hanya boleh mengakses blok memori yang diperuntukkan khas untuknya. Tiada akses langsung ke kernel atau pointer pangkalan data.
2. **Kalis Hentian (*Crash-Proof*):** Jika kod plugin mengalami pembahagian dengan sifar (*divide by zero*) atau gelung infiniti (*infinite loop*), enjin WASM akan mematikan panggilan tersebut secara terkawal tanpa menjatuhkan pelayan TapirusDB.
3. **Bebas Bahasa (*Polyglot Authoring*):** Pembangun boleh menulis sambungan TapirusDB dalam mana-mana bahasa yang menyokong kompilasi WASM (Rust, Go/TinyGo, C++, TypeScript/AssemblyScript, Zig).

---

## 🗺️ Fasa Pelaksanaan Masa Hadapan (*Execution Roadmap*)

* [ ] **Fasa 1: Pengasingan `crates/tapirus-core`**  
  Asingkan struktur B+Tree dan sistem halaman slotted ke dalam crate mikro berasingan untuk membolehkan kompilasi modular.
* [ ] **Fasa 2: Prototaip `tapirus-pgwire` (Port 5432)**  
  Laksanakan pendengar asas PostgreSQL Wire Protocol supaya klien DBeaver dan `psql` boleh membuat sambungan pertama.
* [ ] **Fasa 3: Prototaip `tapirus-mongowire` (Port 27017)**  
  Laksanakan penguraian BSON untuk menerima operasi asas `find()` dan `insertOne()` daripada Mongoose.
* [ ] **Fasa 4: Runtime Plugin WebAssembly**  
  Integrasikan pelaksana WASM (seperti `wasmtime` atau `wasmi`) untuk membolehkan fungsi tentuan pengguna (*User-Defined Functions - UDF*) dimuatkan secara dinamik.
