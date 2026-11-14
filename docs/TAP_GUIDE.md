# Panduan TAP / TapirusDB

Panduan ini menerangkan cara menggunakan TAP tanpa mencampuradukkan tiga perkara yang berbeza:

Versi web panduan ini tersedia di [tapirusdb.com/tap-guide](https://tapirusdb.com/tap-guide).

| Pilihan | Kegunaan | Perlu pasang? |
| --- | --- | --- |
| **TAP Lite** | Cuba klasifikasi dan semakan hipotesis dalam browser | Tidak |
| **TAP native** | Jalankan TAP daripada CLI atau aplikasi Rust | Ya, bina atau pasang TapirusDB |
| **TAP Model Forge** | Latih adapter LoRA menggunakan dataset sendiri | Ya, Studio + Python backend + model tempatan |

## 1. Apa sebenarnya TAP?

TAP ialah helper keputusan tempatan dalam TapirusDB. Ia menyediakan fungsi seperti:

- `classify`: pilih label terbaik daripada beberapa calon;
- `score`: beri skor padanan terhadap kriteria;
- `verify`: bandingkan premise dengan hypothesis;
- fungsi grounded yang boleh menggabungkan rujukan daripada indeks vektor.

TAP berjalan secara tempatan dan bukan chatbot autoregresif. Skor TAP ialah isyarat heuristik, bukan kebarangkalian terkalibrasi. `verify` juga bukan bukti fakta, audit keselamatan atau pengganti semakan manusia.

## 2. Cuba TAP Lite dalam browser

Ini cara paling cepat dan tidak memerlukan pemasangan.

1. Buka [Tapirus Studio Lite](https://tapirusdb.com/studio/).
2. Tunggu status **Loading Rust engine** bertukar kepada engine yang sedia digunakan.
3. Dalam panel **TAP Lite**, isi **Text to classify**.
4. Masukkan label calon dalam **Candidate labels**, dipisahkan dengan koma.
5. Klik **Classify with TAP**.
6. Untuk semakan hipotesis, isi **Premise** dan **Hypothesis**, kemudian klik **Check hypothesis**.

Contoh:

```text
Text to classify:
Saya mahu pulangkan barang yang rosak.

Candidate labels:
refund, support, account
```

Contoh semakan:

```text
Premise:
Data disimpan dalam peranti.

Hypothesis:
Data disimpan secara tempatan.
```

Studio Lite menggunakan enjin Rust WASM dalam pelayar dengan sokongan snapshot tempatan IndexedDB serta API penyulitan XChaCha20-Poly1305 (`TapirusWasm.new_encrypted` / `import_encrypted_bytes`). Simpan backup `.tapir` anda apabila perlu memindahkan pangkalan data ke CLI atau SDK native.

### Versi offline

Di halaman [Releases](https://github.com/tapiruslab/TapirusDB/releases) atau [Downloads](https://tapirusdb.com/download), muat turun arkib pelayar terkini (versi `1.0.5+`):

```text
Tapirus-Studio-Lite-Browser-1.0.5.zip
```

Extract ZIP itu, kemudian jalankan:

```text
Windows:   Start-Tapirus-Lite.cmd
macOS:     sh Start-Tapirus-Lite.sh
Linux:     sh Start-Tapirus-Lite.sh
```

ZIP ini memerlukan Python 3 untuk local HTTP server. Jangan buka `index.html` dengan `file://` kerana browser boleh menyekat WASM atau IndexedDB.

## 3. Jalankan TAP native melalui CLI

### Keperluan

- Rust 1.88 atau lebih baharu (MSRV rasmi);
- source repository TapirusDB;
- Windows PowerShell, atau shell macOS/Linux.

Dari root repository, bina CLI:

```bash
cargo build --release --bin tapirus
```

Selepas binaan selesai, semak bantuan:

```text
Windows:   .\target\release\tapirus.exe --help
macOS/Linux: ./target/release/tapirus --help
```

### Klasifikasi

Windows:

```powershell
.\target\release\tapirus.exe bitnet "Saya mahu refund barang rosak" refund support akaun
```

macOS/Linux:

```bash
./target/release/tapirus bitnet "Saya mahu refund barang rosak" refund support akaun
```

TAP akan memilih satu label daripada calon yang diberikan dan memaparkan skor heuristik.

### Semakan premise dan hypothesis

Windows:

```powershell
.\target\release\tapirus.exe bitnet verify "Data disimpan dalam peranti" "Data disimpan secara tempatan"
```

macOS/Linux:

```bash
./target/release/tapirus bitnet verify "Data disimpan dalam peranti" "Data disimpan secara tempatan"
```

Gunakan arahan bantuan untuk melihat bentuk arahan yang tersedia:

```text
tapirus bitnet --help
```

Arahan `bitnet` ialah demo CLI untuk keputusan tempatan. Ia tidak melatih model dan tidak bermaksud keputusan itu telah disahkan sebagai fakta.

## 4. Guna TAP dalam aplikasi Rust

`TapEngine` boleh digunakan terus, atau diambil daripada `Connection` TapirusDB.

### Dengan `TapEngine`

```rust
use tapirus::{Result, TapEngine};

fn main() -> Result<()> {
    let tap = TapEngine::default();

    let classification = tap.classify(
        "Saya mahu refund barang rosak",
        &["refund", "support", "akaun"],
    )?;
    println!("label: {}", classification.top_choice);
    println!("score: {}", classification.confidence);

    let verification = tap.verify(
        "Data disimpan dalam peranti",
        "Data disimpan secara tempatan",
    )?;
    println!("match: {}", verification.is_verified);
    println!("score: {}", verification.confidence);

    Ok(())
}
```

### Dengan `Connection`

```rust
use tapirus::{Connection, Result};

fn main() -> Result<()> {
    let db = Connection::open("demo.tapir")?;
    let tap = db.tap();

    let result = tap.classify(
        "Saya mahu refund barang rosak",
        &["refund", "support", "akaun"],
    )?;
    println!("{}", result.top_choice);

    db.close()?;
    Ok(())
}
```

Jika aplikasi menerima input pengguna, gunakan parameter SQL untuk data SQL. Jangan anggap skor TAP sebagai authorization, policy enforcement tunggal atau pengesahan keselamatan.

## 5. TAP Model Forge untuk latihan LoRA

Model Forge ialah laluan berasingan untuk melatih adapter model menggunakan dataset sendiri. Ia bukan syarat untuk menggunakan TAP Lite atau arahan `bitnet`.

### Keperluan

- Node.js 20 atau lebih baharu;
- npm;
- Python 3;
- model asas yang boleh dimuat turun dan disimpan secara tempatan;
- ruang disk dan RAM/GPU yang mencukupi untuk model pilihan.

### Pasang Studio dan backend

Dari folder `studio`:

```bash
cd studio
npm ci
python -m pip install -r requirements-training.txt
```

Untuk Windows, boleh gunakan virtual environment:

```powershell
cd studio
python -m venv .venv
.\.venv\Scripts\Activate.ps1
python -m pip install --upgrade pip
python -m pip install -r requirements-training.txt
npm ci
```

### Jalankan Studio

Pilihan Windows yang mudah:

```text
run_studio.bat
```

Atau jalankan secara manual dalam dua terminal.

Terminal 1, backend:

```bash
cd studio
python server.py
```

Terminal 2, antaramuka:

```bash
cd studio
npm run dev
```

Buka URL Vite yang dipaparkan, biasanya `http://localhost:5173`.

### Sediakan dataset

Model Forge menerima `.jsonl`, `.ndjson`, `.json` dan `.csv`. Setiap rekod perlu mempunyai salah satu bentuk medan berikut:

```json
{"instruction":"Bagaimana buat backup?","output":"Gunakan snapshot .tapir dan uji restore."}
```

Bentuk nama medan yang diterima:

- `instruction` + `output`;
- `prompt` + `response`;
- `input` + `target`.

Minimum dua contoh yang sah diperlukan. Data akan dinormalisasi ke folder `studio/training_data/`.

### Aliran Model Forge

1. Buka Studio dan pergi ke Model Forge.
2. Import dataset.
3. Pilih student model atau masukkan Hugging Face repository ID.
4. Download base model ke cache tempatan melalui UI.
5. Pilih parameter latihan.
6. Mulakan latihan sebenar.
7. Semak train loss dan held-out loss.
8. Download bundle `.tap-model.zip` selepas manifest disahkan.

Bundle LoRA memerlukan base model yang dinyatakan dalam manifest. Adapter sahaja tidak boleh digunakan tanpa base modelnya.

### Teacher luar

Teacher OpenAI-compatible adalah pilihan. Jika diaktifkan, contoh dataset yang dipilih boleh dihantar ke API luar untuk melengkapkan output yang hilang. Aktifkan hanya selepas memahami dan memberi consent. Credential tidak sepatutnya dimasukkan ke dataset atau bundle.

### Perkara penting tentang `setup_models`

`setup_models.bat` dan `setup_models.sh` ialah helper untuk memasang Ollama dan memuat turun model Ollama. Ia bukan pengganti kepada:

```bash
python -m pip install -r requirements-training.txt
python server.py
```

Untuk latihan LoRA sebenar, Python backend, dependencies latihan dan base model cache tetap diperlukan.

## 6. Troubleshooting

### Browser menunjukkan engine tidak siap

- Pastikan Studio dibuka melalui `https://` atau local HTTP server.
- Untuk ZIP offline, gunakan `Start-Tapirus-Lite.cmd` atau `Start-Tapirus-Lite.sh`.
- Jangan buka terus dengan `file://`.
- Cuba browser moden seperti Chrome, Edge, Firefox atau Safari.

### Model Forge tiada model yang ready

- Pastikan `python server.py` sedang berjalan.
- Pastikan base model sudah lengkap dalam `studio/model_cache/`.
- Pastikan `config.json` dan model weights wujud.
- Pastikan adapter dan `.tap-model.zip` belum dipadam dari `studio/training_artifacts/`.

### Butang latihan disabled

Biasanya salah satu perkara ini belum selesai:

- dependencies Python belum dipasang;
- backend `server.py` belum berjalan;
- dataset kurang daripada dua contoh sah;
- base model belum selesai dimuat turun;
- model yang dipilih tidak serasi dengan dependencies atau hardware.

### Keputusan TAP nampak pelik

TAP menggunakan heuristik dan baseline weights deterministik secara lalai. Ia bukan model bahasa umum dan bukan sistem pengesahan fakta. Uji dengan contoh domain anda, simpan input/output, dan gunakan semakan manusia untuk keputusan penting.

## 7. Rujukan source

- Panduan ringkas seni bina: [`docs/tapirusdb-guide.html`](tapirusdb-guide.html)
- Studio dan Model Forge: [`studio/README.md`](../studio/README.md)
- Keperluan latihan: [`studio/requirements-training.txt`](../studio/requirements-training.txt)
- API TAP: [`src/tap/mod.rs`](../src/tap/mod.rs)
- Runtime model: [`src/tap/runtime.rs`](../src/tap/runtime.rs)
- CLI TAP: [`src/bin/tapirus.rs`](../src/bin/tapirus.rs)
- Browser WASM workspace: [`crates/tapirus-wasm/README.md`](../crates/tapirus-wasm/README.md)

Panduan ini menerangkan penggunaan yang tersedia dalam source semasa. Ia tidak mendakwa bahawa TAP mempunyai ketepatan, latency, keselamatan atau pensijilan universal untuk semua workload.
