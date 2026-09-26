# Deep Architecture Guide: Dynamic SIMD Vector Acceleration & RaBitQ Quantization
==================================================================================

> **Author**: Ahmad Faiz • Tapirus Tech Lab  
> **Status**: Production Reference Specification  
> **Target Audience**: AI Engineers, High-Performance Computing (HPC) Specialists, and Core Contributors

---

## 1. Executive Summary

Vector search in TapirusDB is not an external plug-in; it is built directly into the storage engine's execution pipeline. By combining **Dynamic 16-Lane SIMD Vector Kernels**, **Fast Walsh-Hadamard Random Rotation (FWHT)**, and **RaBitQ 1-bit / 2-bit Quantization**, TapirusDB reduces memory footprint by up to **32x** while evaluating vector distances in **single-cycle hardware operations**.

```
  High-Dim Float32 Vector (e.g. 1536 Dimensions = 6,144 Bytes)
                           │
                           ▼
  Fast Walsh-Hadamard Transform (FWHT) Random Rotation
                           │
                           ▼
  RaBitQ 1-Bit Sign Quantization (1536 Bits = 192 Bytes + 4B Norm)
                           │
                           ▼
  1-Cycle POPCNT (Hamming Distance) via AVX-512 / AVX2 / ARM NEON
```

---

## 2. Dynamic 16-Lane SIMD Distance Kernels

TapirusDB dynamically detects CPU vector instruction extensions at runtime without requiring recompilation:

* **x86_64**: AVX-512F / AVX2 + FMA (`_mm256_fmadd_ps`, `_mm512_fmadd_ps`)
* **AArch64 / Apple Silicon / Raspberry Pi**: ARM NEON (`vfmaq_f32`, `vdotq_s32`)
* **WebAssembly (WASM)**: SIMD128 (`v128`)
* **Generic Fallback**: Auto-vectorized 4-way loop unrolling in pure safe Rust.

### Distance Metrics Implemented
1. **Cosine Similarity**:
   $$\text{Cosine}(u, v) = \frac{\sum_{i=1}^D u_i v_i}{\|u\|_2 \cdot \|v\|_2}$$
   Vectors can be pre-normalized on insertion, converting Cosine distance to a pure dot product.
2. **Euclidean (L2) Distance**:
   $$\text{L2}^2(u, v) = \sum_{i=1}^D (u_i - v_i)^2$$
3. **Inner Product (Dot Product)**:
   $$\text{IP}(u, v) = \sum_{i=1}^D u_i v_i$$

---

## 3. RaBitQ: 32x Memory Compression with Theoretical Guarantees

In large-scale AI agent deployments, storing full 32-bit floating-point vectors causes massive RAM inflation:
* $100,000 \text{ vectors} \times 1536 \text{ dims} \times 4 \text{ bytes} = 614.4 \text{ MB RAM}$

TapirusDB employs **Randomized Bit Quantization (RaBitQ)**:

### 3.1 Step 1: FWHT Random Rotation
To prevent coordinate bias and catastrophic error cancellation in dense clusters, the vector $v \in \mathbb{R}^D$ is multiplied by an orthogonal random rotation matrix $R$ generated via the Fast Walsh-Hadamard Transform:
$$\tilde{v} = R \cdot v$$
FWHT runs in $O(D \log D)$ time using only additions and subtractions (zero matrix multiplications).

### 3.2 Step 2: 1-Bit / 2-Bit Sign Binarization
Each rotated coordinate $\tilde{v}_i$ is mapped to a single bit:
$$b_i = \begin{cases} 1 & \text{if } \tilde{v}_i \ge 0 \\ 0 & \text{if } \tilde{v}_i < 0 \end{cases}$$
The original vector norm $\|v\|_2$ is stored as a single 32-bit float metadata header.

### 3.3 Step 3: Single-Cycle POPCNT Distance
The inner product between a query vector $q$ and a quantized database vector $b$ is estimated directly via bitwise XOR and hardware population count:
$$\text{Dist}(q, b) \approx \alpha \cdot \text{POPCNT}(q_{bin} \oplus b) + \beta$$
On modern x86 and ARM silicon, `POPCNT` executes in **1 clock cycle**, allowing over **50 million distance evaluations per second per CPU core**.

---

## 4. Disk-Backed Paged HNSW Graph Index

TapirusDB indexes vectors using a multi-layer **Hierarchical Navigable Small World (HNSW)** graph structure:

```
  Layer 2 (Sparse Skip-List):      [Node A] ──────────────────────────► [Node Z]
                                      │                                    │
  Layer 1 (Medium Granularity):    [Node A] ────────► [Node M] ───────► [Node Z]
                                      │                  │                 │
  Layer 0 (Dense Ground Layer):    [Node A] ─► [Node B] ─► [Node M] ─► [Node Q] ─► [Node Z]
```

### 4.1 Page-Aligned HNSW Serialization
Unlike in-memory vector stores that allocate individual heap nodes across random memory addresses, TapirusDB serializes HNSW graph vertices directly into 4KB pages:
* **Node Header**: Vector ID, Layer count, RaBitQ quantized bit-array, Norm.
* **Adjacency Lists**: Contiguous arrays of neighbor vector IDs for layers $0 \dots L$.
* **Cache Locality**: Visiting a vector's neighbors during graph traversal loads adjacent nodes within the same 4KB buffer page, eliminating random SSD seek overhead.
