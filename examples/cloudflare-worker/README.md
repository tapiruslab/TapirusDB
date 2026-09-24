# ⚡ TapirusDB on Cloudflare Workers (Serverless Edge)

> **Deploy sub-5ms cold-start AI database and accelerated GraphRAG directly to 300+ global Cloudflare Edge data centers.**

TapirusDB is architected in 100% pure Safe-Rust with zero external server daemons. Running in V8 isolates, it provides lightning-fast edge database and AI memory without requiring backend servers.

---

## 🌟 Capabilities

- **Zero Cold Start:** Starts execution in $< 5\text{ ms}$ (no TCP connection handshake or database daemon booting).
- **Accelerated GraphRAG Endpoint:** Run `POST /graph-rag` with Product Quantization seed matching and micro-hop graph traversal to return prompt-ready markdown for edge SLMs.
- **Embedded SQL:** Execute standard SQL queries (`CREATE`, `INSERT`, `SELECT`) directly in worker isolates.
- **AI Agent Memory:** Ingest dialogue and facts with temporal recency decay and associative memory.

---

## 🚀 Quick Start

### 1. Install Wrangler CLI
```bash
npm install -g wrangler
```

### 2. Run Locally
```bash
wrangler dev
```

The worker will start on `http://localhost:8787`.

---

## 📡 API Endpoints

### 1. Health & Engine Status
```bash
curl http://localhost:8787/health
```

**Response:**
```json
{
  "status": "online",
  "engine": "TapirusDB Edge",
  "version": "1.0.0",
  "runtime": "Cloudflare Workers (V8 Isolate)"
}
```

### 2. Accelerated GraphRAG Query
```bash
curl -X POST http://localhost:8787/graph-rag \
  -H "Content-Type: application/json" \
  -d '{
    "query": "Quantum Machine Learning",
    "topSeeds": 3,
    "maxHops": 2,
    "limit": 5
  }'
```

**Response:**
```json
{
  "success": true,
  "query": "Quantum Machine Learning",
  "entities": [
    {
      "entityId": 101,
      "label": "SystemNode",
      "rrfScore": 0.0425,
      "hopDistance": 0,
      "relatedEdges": []
    }
  ],
  "promptContext": "### 🧠 Verified Knowledge Graph Context\n..."
}
```

### 3. Execute SQL Query
```bash
curl -X POST http://localhost:8787/sql \
  -H "Content-Type: application/json" \
  -d '{
    "query": "INSERT INTO system_logs (id, level, message, created_at) VALUES (1, '\''INFO'\'', '\''Edge node online'\'', 1726848000);"
  }'
```

---

## 🚢 Deploy to Production

Deploy globally to Cloudflare's edge network with a single command:

```bash
wrangler deploy
```
