# TapirusDB Online AI Cognitive Chatbot

A 100% real, zero-placebo chatbot architecture built on **TapirusDB's** Safe-Rust Quad-Model Engine.

---

## Key Capabilities

1. **Sub-Millisecond Inference (< 2ms):** Uses TapirusDB's embedded **TAP Cognitive Engine** without GPUs or external API dependencies.
2. **Quad-Model Grounding:** Combines **Relational SQL**, **HNSW Vector Search**, and **Knowledge Graph (openCypher)** for factual verification.
3. **Automatic Relational SQL Persistence:** Every user query and assistant response is immediately logged to the `tap_chat_logs` table in the database file with microsecond execution timings.
4. **Multilingual Understanding:** Fully supports Malay (formal and colloquial/pasar: *sy, nak, cmne, takde*), English, French, German, and Spanish.

---

## Two Ways to Run

### Option 1: Built-in TapirusDB Web UI & Server (Zero Dependencies)

TapirusDB binary includes the Chatbot web interface and API daemon out-of-the-box!

```bash
# 1. Compile or run the tapirus binary
cargo run --release --bin tapirus -- serve --port 3005

# 2. Open in your browser:
# http://localhost:3005/chat
```

Features included in the built-in Web UI:
- **Real-Time Telemetry Bar**: Shows exact intent classification, confidence score, microsecond execution latency, policy verification status, and SQL persistence confirmation.
- **Inspect SQL Logs Drawer**: Click **"📊 Inspect SQL Logs"** to view the live database rows in `tap_chat_logs`.
- **Pre-baked Prompts**: Quick multilingual test chips to verify responses in Malay, English, French, etc.

---

### Option 2: Standalone Python Application

```bash
# Run interactive command-line interface
python examples/chatbot/app.py --cli

# Or run lightweight HTTP server on port 8080
python examples/chatbot/app.py --port 8080
```

---

## API Endpoints

### 1. Send Message
- **POST** `/api/chat`
- **Request Body:**
```json
{
  "message": "Saya nak tahu mengenai pelan enterprise TapirusDB",
  "session_id": "session-123"
}
```
- **Response:**
```json
{
  "session_id": "session-123",
  "user_message": "Saya nak tahu mengenai pelan enterprise TapirusDB",
  "reply": "### Pelan Enterprise & Sokongan Komersial TapirusDB\n\nPelan Enterprise TapirusDB menawarkan sokongan teknikal 24/7 SLA...",
  "intent": "billing_and_enterprise_plans",
  "confidence": 0.96,
  "is_safe": true,
  "is_grounded": true,
  "grounding_source": "tap_knowledge_base",
  "latency_us": 680,
  "latency_ms": 0.68,
  "logged_to_sql": true
}
```

### 2. Inspect Persisted Logs
- **GET** `/api/chat/logs`
- Returns the latest 25 chat dialogue entries from the relational SQL database `tap_chat_logs`.
