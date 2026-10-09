"""
TapirusDB Native AI Cognitive Chatbot (Zero-Placebo Implementation)
===================================================================
A complete, runnable, production-ready chatbot powered directly by TapirusDB's
Safe-Rust Quad-Model Engine:
  1. TAP Cognitive Perception (Intent classification & policy verification < 2ms)
  2. Relational SQL persistence (Automatic dialogue logging in `tap_chat_logs`)
  3. Knowledge Retrieval (Grounded factual Q&A in `tap_knowledge_base`)
  4. Multilingual support (Bahasa Melayu, English, French, German, Spanish)

Usage:
  Interactive CLI Mode:
    python app.py --cli

  HTTP Web Server Mode:
    python app.py --port 8080
"""

import sys
import os
import time
import json
import http.server
import socketserver
import urllib.parse
from typing import Dict, Any, Tuple, Optional

# Attempt to load native Python SDK or fallback to TapirusDB HTTP daemon
sys.path.insert(0, os.path.abspath(os.path.join(os.path.dirname(__file__), "../../python")))
try:
    import tapirus
    HAS_NATIVE_SDK = True
except ImportError:
    HAS_NATIVE_SDK = False

DB_PATH = "chatbot_memory.tapir"

# Default factual knowledge base to seed if running standalone
SEEDED_KNOWLEDGE = [
    {
        "category": "tap_cognitive_engine",
        "keywords": "tap perception cognitive inference latency sub-millisecond intent classify nli verify",
        "title": "TAP Sub-Millisecond Cognitive Engine",
        "content": "TAP (Tapirus Accelerated Perception) is TapirusDB's embedded cognitive perception engine. It delivers sub-millisecond (< 2ms) intent classification, NLI policy verification, and semantic routing natively in 100% Safe Rust without external GPUs or heavy Python runtimes.",
        "language": "en"
    },
    {
        "category": "database_architecture_and_rag",
        "keywords": "architecture quad-model sql vector hnsw graph opencypher json rag embedded acid",
        "title": "TapirusDB Quad-Model Architecture",
        "content": "TapirusDB unifies Relational SQL, native HNSW Vector search, openCypher Knowledge Graph, and Schemaless Documents into a single embedded engine with ACID compliance and zero external dependencies.",
        "language": "en"
    },
    {
        "category": "billing_and_enterprise_plans",
        "keywords": "billing enterprise plans pricing commercial license sla support cluster",
        "title": "TapirusDB Enterprise & Licensing Plans",
        "content": "TapirusDB is open-source under BUSL-1.1 for development. The Enterprise Plan provides dedicated 24/7 SLA production support, multi-node clustering replication, custom GraphRAG tuning, and commercial production licensing.",
        "language": "en"
    },
    {
        "category": "database_architecture_and_rag",
        "keywords": "pangkalan data seni bina quad-model vektor hnsw graf opencypher dokumen sql rag melayu",
        "title": "Seni Bina Quad-Model TapirusDB",
        "content": "TapirusDB adalah pangkalan data terbenam (embedded) berprestasi tinggi dalam Safe Rust yang menggabungkan SQL Relasi, carian Vektor HNSW, Graf Pengetahuan openCypher, dan Dokumen JSON dalam satu fail tunggal yang patuh ACID tanpa kebergantungan luar.",
        "language": "ms"
    },
    {
        "category": "tap_cognitive_engine",
        "keywords": "tap enjin kognitif niat klasifikasi verifikasi sub-milisaat ai memori melayu",
        "title": "Enjin Kognitif TAP Sub-Milisaat",
        "content": "Enjin TAP (Tapirus Accelerated Perception) memproses klasifikasi niat (intent) dan semakan polisi (verification) dalam masa kurang 2 milisaat (< 2ms) terus dalam pangkalan data tanpa memerlukan GPU atau persekitaran Python luaran.",
        "language": "ms"
    },
    {
        "category": "billing_and_enterprise_plans",
        "keywords": "pelan langganan enterprise harga bayaran sokongan sla lesen komersial beli pakej",
        "title": "Pelan Enterprise & Sokongan Komersial TapirusDB",
        "content": "Pelan Enterprise TapirusDB menawarkan sokongan teknikal 24/7 SLA, lesen komersial penuh, kluster replikasi teragih, bantuan penalaan GraphRAG tersuai, dan penyulitan ChaCha20-Poly1305 gred industri. Hubungi sales@tapirusdb.com untuk maklumat lanjut.",
        "language": "ms"
    }
]

def sql_escape(s: str) -> str:
    return s.replace("'", "''")

def detect_language(text: str) -> str:
    lower = text.lower()
    words = lower.split()
    ms_markers = {"saya", "sy", "nak", "nk", "boleh", "bagaimana", "macam", "cmne", "apa", "cara", "pelan", "harga", "tukar", "tolong", "ada", "ini", "tu", "dan", "ke", "di", "hai", "khabar", "guna", "buat"}
    fr_markers = {"bonjour", "comment", "merci", "avec", "pour", "votre", "base", "donnees"}
    de_markers = {"hallo", "wie", "danke", "bitte", "datenbank", "kann", "ich", "brauche"}
    es_markers = {"hola", "como", "gracias", "por", "favor", "para", "cuenta", "base"}

    ms_score = sum(1 for w in words if w in ms_markers)
    fr_score = sum(1 for w in words if w in fr_markers)
    de_score = sum(1 for w in words if w in de_markers)
    es_score = sum(1 for w in words if w in es_markers)

    if ms_score > 0 and ms_score >= max(fr_score, de_score, es_score):
        return "ms"
    if fr_score > 0 and fr_score >= max(de_score, es_score):
        return "fr"
    if de_score > 0 and de_score >= es_score:
        return "de"
    if es_score > 0:
        return "es"
    return "en"

class TapirusChatbot:
    def __init__(self, db_path: str = DB_PATH):
        self.db_path = db_path
        self.conn = None
        self._init_database()

    def _init_database(self):
        if HAS_NATIVE_SDK:
            self.conn = tapirus.connect(self.db_path)
            # Create dialogue logs table
            self.conn.execute("""
                CREATE TABLE IF NOT EXISTS tap_chat_logs (
                    id INTEGER PRIMARY KEY,
                    session_id TEXT,
                    user_message TEXT,
                    bot_reply TEXT,
                    intent TEXT,
                    confidence REAL,
                    is_safe INTEGER,
                    latency_us INTEGER,
                    created_at TEXT
                );
            """)
            # Create knowledge table
            self.conn.execute("""
                CREATE TABLE IF NOT EXISTS tap_knowledge_base (
                    id INTEGER PRIMARY KEY,
                    category TEXT,
                    keywords TEXT,
                    title TEXT,
                    content TEXT,
                    language TEXT
                );
            """)
            # Seed knowledge base
            res = self.conn.query("SELECT COUNT(*) FROM tap_knowledge_base;")
            count = 0
            if res and isinstance(res, list) and len(res) > 0:
                count = list(res[0].values())[0] if res[0] else 0

            if count == 0:
                for idx, item in enumerate(SEEDED_KNOWLEDGE, start=1):
                    sql = f"""
                        INSERT INTO tap_knowledge_base (id, category, keywords, title, content, language)
                        VALUES ({idx}, '{sql_escape(item['category'])}', '{sql_escape(item['keywords'])}', 
                                '{sql_escape(item['title'])}', '{sql_escape(item['content'])}', '{sql_escape(item['language'])}');
                    """
                    self.conn.execute(sql)
                self.conn.checkpoint()

    def ask(self, message: str, session_id: str = "default-session") -> Dict[str, Any]:
        start_time = time.perf_counter_ns()
        lang = detect_language(message)

        candidate_intents = [
            "database_architecture_and_rag",
            "tap_cognitive_engine",
            "python_and_developer_sdk",
            "security_and_encryption",
            "billing_and_enterprise_plans",
            "general_greeting_or_help"
        ]

        if HAS_NATIVE_SDK and self.conn:
            # 1. Native TAP Classification
            intent, confidence = tapirus.tap_classify(message, candidate_intents)
            # 2. Native TAP Verification
            is_safe = tapirus.tap_verify("User message is a safe constructive technical inquiry.", message)
            
            # 3. Grounded retrieval
            if not is_safe:
                reply = "Mesej anda ditandakan oleh pengesahan polisi keselamatan TAP." if lang == "ms" else "Your message was flagged by TAP policy verification."
                grounding = "tap_policy_guardrail"
            elif intent == "general_greeting_or_help":
                reply = (
                    "Hai! Saya pembantu pintar TapirusDB. Dikuasakan oleh enjin kognitif TAP sub-milisaat (<2ms), "
                    "saya bersedia membantu anda mengenai SQL, carian Vektor HNSW, Graf Pengetahuan openCypher, "
                    "atau pelan Enterprise TapirusDB. Ada apa yang boleh saya bantu?"
                    if lang == "ms" else
                    "Hello! I am your TapirusDB cognitive assistant powered by our sub-millisecond (<2ms) TAP engine. "
                    "How can I help you today with SQL, Vector search, Knowledge Graphs, or Enterprise plans?"
                )
                grounding = "tap_conversational_core"
            else:
                sql = f"SELECT title, content FROM tap_knowledge_base WHERE category = '{sql_escape(intent)}' AND (language = '{lang}' OR language = 'en');"
                rows = self.conn.query(sql)
                if rows and len(rows) > 0:
                    r = rows[0]
                    title = r.get("title", "TapirusDB Verified Knowledge")
                    content = r.get("content", "")
                    reply = f"**{title}**\n\n{content}\n\n*(Disahkan melalui tap_knowledge_base)*"
                    grounding = "tap_knowledge_base"
                else:
                    reply = "TapirusDB adalah pangkalan data berbilang model terbenam dalam 100% Safe Rust." if lang == "ms" else "TapirusDB is an embedded quad-model database engine in Safe Rust."
                    grounding = "tap_default"

            end_time = time.perf_counter_ns()
            latency_us = int((end_time - start_time) / 1000)

            # 4. Relational SQL Persistence
            now_ts = str(int(time.time()))
            insert_sql = f"""
                INSERT INTO tap_chat_logs (session_id, user_message, bot_reply, intent, confidence, is_safe, latency_us, created_at)
                VALUES ('{sql_escape(session_id)}', '{sql_escape(message)}', '{sql_escape(reply)}', '{sql_escape(intent)}', 
                        {confidence:.4f}, {1 if is_safe else 0}, {latency_us}, '{now_ts}');
            """
            self.conn.execute(insert_sql)
            self.conn.checkpoint()

            return {
                "reply": reply,
                "intent": intent,
                "confidence": round(confidence, 4),
                "is_safe": is_safe,
                "latency_us": latency_us,
                "latency_ms": round(latency_us / 1000.0, 2),
                "grounding_source": grounding,
                "session_id": session_id,
                "logged_to_sql": True
            }
        else:
            # Fallback to TapirusDB HTTP Daemon on localhost:3005
            import urllib.request
            req_data = json.dumps({"message": message, "session_id": session_id}).encode("utf-8")
            req = urllib.request.Request(
                "http://127.0.0.1:3005/api/chat",
                data=req_data,
                headers={"Content-Type": "application/json"}
            )
            with urllib.request.urlopen(req) as resp:
                return json.loads(resp.read().decode("utf-8"))

    def get_recent_logs(self, limit: int = 10):
        if HAS_NATIVE_SDK and self.conn:
            return self.conn.query(f"SELECT * FROM tap_chat_logs ORDER BY id DESC LIMIT {limit};")
        return []

def run_cli():
    print("=" * 70)
    print("  🦣 TapirusDB AI Cognitive Chatbot (Zero-Placebo Native Engine)")
    print("  Supported Languages: English, Bahasa Melayu, French, German, Spanish")
    print("  Type 'exit' to quit | Type 'logs' to inspect real SQL persistence")
    print("=" * 70)
    
    bot = TapirusChatbot()
    session_id = f"cli-{int(time.time())}"

    while True:
        try:
            user_input = input("\nYou: ").strip()
            if not user_input:
                continue
            if user_input.lower() in ("exit", "quit"):
                print("Terima kasih & Goodbye!")
                break
            if user_input.lower() == "logs":
                logs = bot.get_recent_logs(5)
                print("\n📊 Recent Relational SQL Logs (tap_chat_logs):")
                print(json.dumps(logs, indent=2))
                continue

            res = bot.ask(user_input, session_id=session_id)
            print(f"\nAssistant:\n{res['reply']}")
            print(f"\n[Telemetry: Intent={res['intent']} ({int(res['confidence']*100)}%) | Latency={res['latency_us']} µs ({res['latency_ms']} ms) | Stored in SQL: tap_chat_logs]")

        except (KeyboardInterrupt, EOFError):
            print("\nSession ended.")
            break

class ChatbotHTTPHandler(http.server.BaseHTTPRequestHandler):
    chatbot = TapirusChatbot()

    def do_POST(self):
        if self.path in ("/api/chat", "/chat"):
            length = int(self.headers.get("Content-Length", 0))
            body = self.rfile.read(length).decode("utf-8")
            try:
                payload = json.loads(body)
                msg = payload.get("message", "")
                sess = payload.get("session_id", "web-session")
                res = self.chatbot.ask(msg, sess)
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Access-Control-Allow-Origin", "*")
                self.end_headers()
                self.wfile.write(json.dumps(res).encode("utf-8"))
            except Exception as e:
                self.send_response(400)
                self.send_header("Content-Type", "application/json")
                self.end_headers()
                self.wfile.write(json.dumps({"error": str(e)}).encode("utf-8"))
        else:
            self.send_response(404)
            self.end_headers()

    def do_GET(self):
        if self.path in ("/api/chat/logs", "/chat/logs"):
            logs = self.chatbot.get_recent_logs(20)
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Access-Control-Allow-Origin", "*")
            self.end_headers()
            self.wfile.write(json.dumps({"logs": logs}).encode("utf-8"))
        else:
            self.send_response(200)
            self.send_header("Content-Type", "text/plain")
            self.end_headers()
            self.wfile.write(b"TapirusDB Chatbot Python Server Active. POST to /api/chat or GET /api/chat/logs")

def run_server(port: int = 8080):
    with socketserver.TCPServer(("", port), ChatbotHTTPHandler) as httpd:
        print(f"🚀 TapirusDB Chatbot Python server running at http://localhost:{port}")
        print("Press Ctrl+C to stop.")
        try:
            httpd.serve_forever()
        except KeyboardInterrupt:
            print("\nShutting down server.")

if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "--cli":
        run_cli()
    elif len(sys.argv) > 1 and sys.argv[1] == "--port":
        port = int(sys.argv[2]) if len(sys.argv) > 2 else 8080
        run_server(port)
    else:
        # Default to CLI mode
        run_cli()
