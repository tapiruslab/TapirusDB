# PELAN INDUK & SPESIFIKASI TEKNIKAL: TAPIRUS AI GURU & STUDIO PRO
**Versi:** 1.0.0-PRO  
**Klasifikasi:** Blueprint Produk Komersial & Kejuruteraan Sistem  
**Matlamat Utama:** Pendemokrasian latihan dan pemilikan model bahasa kecil (SLM) tempatan pada komputer biasa (RAM 8GB–16GB) tanpa kos API awam.

---

## 1. Visi Produk & Kedudukan Pasaran

Kebanyakan perusahaan kecil dan sederhana (PKS), institusi pendidikan, klinik, dan agensi kerajaan menghadapi 3 kekangan utama untuk menggunakan AI:
1. **Kos API Awam Melambung Tinggi:** Bergantung kepada langganan token bulanan (OpenAI, Anthropic, Google Cloud).
2. **Kebocoran Data Sensitif:** Data rahsia syarikat atau rekod peribadi dihantar ke pelayan awan luar negara.
3. **Monopoli Perkakasan Gergasi:** Anggapan salah bahawa membina AI memerlukan kluster pelayan GPU bernilai ratusan ribu ringgit.

**Tapirus AI Guru** mengubah paradigma ini:
> *"Jadikan setiap PC biasa sebagai Sekolah AI dan Makmal Penyelidikan Tempatan."*

Dengan memanfaatkan pangkalan data berhalaman 4KB terenkripsi **TapirusDB** digabungkan bersama **Model Bahasa Padat (SLM 0.5B – 3B)**, pengguna boleh mendidik model mereka sendiri (**TapirLM-1**) secara percuma, luar talian, dan selamat.

---

## 2. Struktur Peringkat Produk (Product Tiering)

| Komponen | **Tapirus Studio Community (Web)** | **Tapirus Studio Pro (Desktop)** | **Tapirus Enterprise Mesh** |
| :--- | :--- | :--- | :--- |
| **Model Pengedaran** | Percuma (In-Browser WASM) | Berbayar / Lesen Pro (Desktop App) | Langganan Korporat / On-Premise |
| **Pangkalan Data** | TapirusDB WASM (Simpanan Pelayar) | TapirusDB Natif (Fail `.tapir` sehingga ratusan GB) | TapirusDB Teragih (Multi-Node / High Availability) |
| **Kapasiti AI** | Eksplorasi Vektor k-NN, Visual Graf, SQL | **Latihan SLM (LoRA/QLoRA), RAG Lanjutan, Distillation** | **Federated Learning Mesh, Kluster MoE, Audit Korporat** |
| **Akselerasi Perkakasan** | CPU Pelayar (Terhad WebAssembly) | CPU SIMD (AVX2/AVX-512), Apple Silicon Metal, CUDA | Multi-GPU (NVIDIA H100/A100/RTX), NPU Terbenam |
| **Privasi Data** | Bebas Awan (Local Sandbox) | 100% Offline / Air-Gapped | Enkripsi ChaCha20-Poly1305 + Kawalan Akses RBAC |

---

## 3. Seni Bina Aliran Kerja: 6 Fasa "AI Guru"

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│                    ALIRAN KERJA LATIHAN TAPIRUS AI GURU                         │
└─────────────────────────────────────────────────────────────────────────────────┘

 [ FASA 1: KURASI DATA ]
    Pangkalan Data .tapir (Teks, Jadual SQL, Graf Hubungan, Dokumen PDF)
                          │
                          ▼
 [ FASA 2: SINTESIS PENGETAHUAN (AI GURU) ]
    Otak Pemula (Open-Weight SLM 1.5B) menjana soalan & rantaian penaakulan (Chain-of-Thought)
                          │
                          ▼
 [ FASA 3: LATIHAN PENYESUAI TEMPATAN (LoRA/QLoRA ENGINE) ]
    Melatih lapisan pemberat kecil (Adapter ~50MB) pada CPU/RAM PC biasa
                          │
                          ▼
 [ FASA 4: PAKEJ MODEL TAPIRUS (.tapir-model) ]
    Menggabungkan Adapter + Kuantisasi 4-bit GGUF + Skema Vektor ke 1 Fail
                          │
                          ▼
 [ FASA 5: SUAP GABUNGAN PAKAR (MoE ROUTER) ]
    TapirusDB menghalakan pertanyaan pengguna ke SLM Pakar yang betul
                          │
                          ▼
 [ FASA 6: KEPINTARAN BERSEKUTU (FEDERATED AI) ]
    Penyegerakan pembaikan model antara nod tanpa berkongsi data mentah
```

---

## 4. Spesifikasi Ciri Utama (Feature Breakdown)

### A. Modul "Bina Model Sendiri" (Custom Model Wizard)
1. **Penyambung Data Tapirus (Zero-ETL):**
   - Pilih terus jadual SQL, koleksi dokumen, atau graf entiti daripada fail vault `.tapir`.
   - Tiada keperluan mengeksport ke format JSONL rumit secara manual.
2. **Koleksi Otak Pemula (Foundational Base Brains):**
   - Diprapasang dengan model bahasa terbuka berkualiti tinggi yang menyokong Bahasa Melayu & Inggeris:
     - **Qwen 2.5 (0.5B, 1.5B, 3B):** Sangat mahir dalam kod, matematik, dan multibahasa.
     - **SmolLM2 (1.7B):** Ringan, pantas, dan jimat memori.
     - **Llama 3.2 (1B, 3B):** Ekosistem global yang stabil dan serasi.
3. **Penyulingan Alasan Automatik (*Self-Instruct & Synthetic QA*):**
   - Enjin "AI Guru" membaca dokumen pengguna dan secara automatik menjana ribuan pasangan soalan, jawapan, dan langkah logik (*Reasoning Traces*).

### B. Enjin Latihan Ringan Tempatan (Micro-Trainer Engine)
1. **Sokongan QLoRA 4-Bit (Quantized Low-Rank Adaptation):**
   - Berat model asas dibekukan (*frozen base model*) dalam format 4-bit (hanya menggunakan ~1GB – 2GB RAM).
   - Hanya matriks peringkat rendah (*rank matrices*) dilatih, mengurangkan keperluan VRAM/RAM sehingga 90%.
2. **CPU Multithreading & SIMD Acceleration:**
   - Dioptimumkan menggunakan pustaka Rust natif (`candle` atau `llama.cpp-rs`) yang memanfaatkan arahan AVX2 / AVX-512 pada pemproses Intel/AMD biasa dan Apple Silicon NEON.
3. **Penjejak Kemajuan Visual (Real-Time Loss Graph):**
   - Paparan graf interaktif dalam Studio yang menunjukkan penurunan nilai *Loss*, ketepatan jawapan, dan anggaran masa siap.

### C. Hab Gabungan Pakar (*Mixture of Experts - MoE Swarm*)
Daripada membina satu model 70B yang perlahan, pengguna melatih beberapa SLM pakar:
* **TapirLM-Kewangan (1.5B)**
* **TapirLM-Undang2 (1.5B)**
* **TapirLM-KhidmatPelanggan (1.5B)**

**Enjin TapirusDB bertindak sebagai Central Router:**
* Apabila soalan masuk, pengelas berasaskan vektor pantas (*HNSW Router*, masa tindak balas < 2 milisaat) menentukan model pakar mana yang perlu menjawab.
* Hanya satu model diaktifkan pada satu-satu masa, memastikan penggunaan RAM kekal di bawah 2GB sepanjang masa.

### D. Rangka Kerja AI Bersekutu (*Federated AI Protocol*)
* **Privasi Mutlak:** Komputer cawangan (cth: hospital cawangan A dan cawangan B) melatih model mereka secara berasingan. Data pesakit kekal dalam fail `.tapir` cawangan masing-masing.
* **Perkongsian Pemberat Sahaja (*Weight Delta Sync*):** Hanya fail adapter LoRA (~50MB) yang dihantar melalui sambungan selamat TapirusDB P2P.
* **Algoritma FedAvg (Federated Averaging):** Menggabungkan pembelajaran daripada ratusan peranti menjadi satu model induk tanpa sebarang data mentah bocor.

### E. Falsafah Reka Bentuk: Wizard 4-Klik "Plug & Play" (Zero-Terminal, Zero-Code)
Pengguna **TIDAK PERLU** membuka terminal hitam, menaip skrip Python, mengurus persekitaran CUDA rumit, atau memformat fail JSONL secara manual. Keseluruhan proses dipermudahkan kepada 4 langkah grafik intuitif:

* **Langkah 1: Pilih Sumber Pengetahuan (1 Klik)**
  - Paparan visual senarai jadual, koleksi dokumen, atau fail `.tapir` sedia ada.
  - Pengguna hanya perlu menanda kotak (*checkbox*) data yang ingin diajar kepada AI.
* **Langkah 2: Pilih Otak Pemula (1 Klik)**
  - Tiga pilihan kad grafik yang jelas dengan penunjuk keserasian perkakasan:
    - 🟢 **Ringan (0.5B)** — Sangat pantas, sesuai untuk laptop pejabat biasa (RAM 4GB–8GB).
    - 🔵 **Standard (1.5B) [Disyorkan]** — Pintar, seimbang, mahir Bahasa Melayu & Inggeris (RAM 8GB–16GB).
    - 🟣 **Lanjutan (3B)** — Penaakulan logik mendalam untuk analisis kompleks (RAM 16GB).
* **Langkah 3: Namakan & Tekan Butang "Mula Ajar AI" (1 Klik)**
  - Namakan model (cth: `TapirLM-Klinik` atau `TapirLM-Kewangan`).
  - Tekan satu butang utama: **[ 🚀 Mula Mengajar AI ]**.
  - Studio menjalankan segalanya di latar belakang secara automatik:
    - Auto-ekstrak konteks & sintesis soalan-jawapan (*Chain-of-Thought*).
    - Auto-latih adapter mikro QLoRA tanpa membebankan RAM.
    - Paparan visual bar kemajuan (*Progress Bar*) dan anggaran masa siap (*ETA*).
* **Langkah 4: Terus Berbual & Uji Hasil (1 Klik)**
  - Sejurus latihan selesai, tetingkap bual (*Chat Playground*) dibuka secara automatik.
  - Pengguna boleh terus menyoal dan menguji model baharu mereka.
  - Butang 1-klik untuk:
    - **[ 💾 Simpan Model Tunggal (.tapir-model) ]** — Boleh dipindahkan ke PC lain guna pendrive.
    - **[ 🤖 Aktifkan Sebagai Pakar MoE ]** — Bersedia untuk digabungkan dengan model-model lain.

### F. Falsafah Pemasangan: Zero-Registration & Prapasang Automatik (Out-of-the-Box)
Pengalaman kali pertama (*first-time onboarding*) mestilah sebersih perisian desktop klasik (seperti VLC Player atau Notepad++):

1. **Tiada Pendaftaran / Log Masuk (Zero Sign-Up):**
   - Pengguna tidak disekat dengan borang pendaftaran, pengesahan e-mel, atau "Sign in with Google/GitHub". Pasang dan terus terbuka.
2. **Tiada Kunci API / Token Hugging Face (Zero Token Hassle):**
   - Tidak memerlukan sebarang token rahsia atau kad kredit. Model pemula adalah hak milik bebas (*open weights*).
3. **Prapasang & Auto-Load Model Tempatan (Auto-Provisioning):**
   - Pemasang (*Installer*) memuatkan secara automatik satu Otak Pemula Standard (cth: *SLM 0.5B/1.5B 4-bit*) ke dalam storan tempatan PC.
   - Apabila aplikasi dilancarkan buat kali pertama, penunjuk status model terus berwarna hijau: `● Otak Pemula Sedia Digunakan`. Pengguna tidak perlu mencari pautan muat turun manual atau mengurus fail `.gguf` yang mengelirukan.
4. **Beroperasi 100% Luar Talian (Offline-First Guarantee):**
   - Selepas dipasang, pengguna boleh mencabut kabel LAN atau mematikan Wi-Fi. Tapirus Studio Pro dan enjin latihannya boleh berjalan sepenuhnya di persekitaran *air-gapped* tanpa memerlukan sambungan internet.

---

## 5. Bajet Perkakasan & Penggunaan Memori (Hardware Matrix)

| Konfigurasi Model | Saiz Asas (4-Bit) | Penggunaan RAM Latihan (QLoRA) | Penggunaan RAM Inferens | Sasaran Komputer |
| :--- | :--- | :--- | :--- | :--- |
| **SLM Nano (0.5B)** | ~350 MB | **~800 MB** | **~450 MB** | Komputer riba pejabat lama (RAM 4GB–8GB) |
| **SLM Standard (1.5B)** | ~980 MB | **~2.2 GB** | **~1.2 GB** | Laptop / PC biasa (RAM 8GB–16GB, Core i5/Ryzen 5) |
| **SLM Komprehensif (3B)** | ~2.1 GB | **~4.8 GB** | **~2.6 GB** | PC kerja sederhana (RAM 16GB, tanpa GPU diskret) |
| **SLM Kuasa Penuh (7B/8B)** | ~4.6 GB | **~9.5 GB** | **~5.5 GB** | PC berprestasi tinggi (RAM 16GB–32GB atau RTX 3060) |

---

## 6. Format Fail Kontena Model: `.tapir-model`

Bagi memastikan model mudah diedarkan dan dipasang, Tapirus Studio memperkenalkan format tunggal:

```
[ Fail Kontena Tunggal: model_name.tapir-model ]
├── Header & Metadata (Versi, Nama Pengarang, Domain Pengkhususan)
├── Base Model Hash & Quantization Parameters (Q4_K_M)
├── LoRA Adapter Weights (Tensors Bfloat16/Float16)
├── Prompt Template & System Instructions ("Karakter Guru")
├── Embedded HNSW Vector Seed Index (Untuk RAG Tempatan)
└── AEAD Signature & Hash Semakan Integriti
```

Pengguna boleh memindahkan fail ini menggunakan pemacu kilat USB (*thumbdrive*) atau rangkaian tempatan, dan membukanya pada mana-mana komputer yang mempunyai Tapirus Studio.

---

## 7. Pelan Hala Tuju Pembangunan (Milestones & Roadmap)

### Fasa 1: Seni Bina Inferens Tempatan & Kurasi Data (Q4 2026)
* [x] Enjin pangkalan data TapirusDB terbukti stabil dengan indeks vektor HNSW dan carian graf.
* [x] In-Browser Studio Web siap dengan penukar *Quad-Model*.
* [ ] Membina versi Desktop Tapirus Studio (menggunakan Tauri v2 + Rust).
* [ ] Integrasi runtime inferens tempatan GGUF (CPU AVX2 & Apple Metal).

### Fasa 2: Enjin Latihan Mikro LoRA & Modul "AI Guru" (Q1 2027)
* [ ] Pembangunan wizard kurasi data (SQL/Teks/Graf -> Format Latihan).
* [ ] Enjin latihan QLoRA 4-bit menggunakan pustaka Rust `candle`.
* [ ] Modul penjanaan soalan sintetik automatik (*Reasoning Distillation*).
* [ ] Pengesahan latihan model TapirLM-1 (1.5B) pada PC dengan RAM 8GB.

### Fasa 3: Penggabungan Pakar (MoE Swarm Router) (Q2 2027)
* [ ] Enjin penghalaan pantas berasaskan vektor HNSW untuk memilih SLM pakar.
* [ ] Modul penggabungan jawapan berbilang model (*Multi-Agent Orchestration*).
* [ ] Format kontena model tunggal `.tapir-model`.

### Fasa 4: Rangkaian AI Bersekutu & Versi Komersial Enterprise (Q3 2027)
* [ ] Protokol penyegerakan adapter LoRA (*Federated Averaging*).
* [ ] Sistem pelesenan Pro/Enterprise Desktop dan kawalan keselamatan air-gapped.
* [ ] Penerbitan kertas putih dan penanda aras (*Benchmark Report*) Tapirus AI Guru.

---
*Disediakan oleh Pasukan Kejuruteraan TapirusDB untuk perancangan strategik Tapirus Studio Pro.*
