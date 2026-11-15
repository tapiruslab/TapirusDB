use super::*;

pub(super) fn detect_language(text: &str) -> &'static str {
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();

    let ms_markers = [
        "saya",
        "sy",
        "nak",
        "nk",
        "boleh",
        "bagaimana",
        "macam",
        "cmne",
        "apa",
        "cara",
        "pelan",
        "harga",
        "tukar",
        "tolong",
        "ada",
        "ini",
        "tu",
        "dan",
        "ke",
        "di",
        "hai",
        "khabar",
        "guna",
        "buat",
        "pangkalan",
        "data",
        "terbenam",
        "vektor",
        "graf",
        "enjin",
    ];
    let fr_markers = [
        "bonjour",
        "comment",
        "merci",
        "avec",
        "pour",
        "votre",
        "base",
        "donnees",
        "aidez",
        "mot",
        "passe",
        "securite",
        "chiffrement",
    ];
    let de_markers = [
        "hallo",
        "wie",
        "danke",
        "bitte",
        "datenbank",
        "kann",
        "ich",
        "brauche",
        "hilfe",
        "unterstutzung",
        "sicherheit",
        "speicher",
    ];
    let es_markers = [
        "hola",
        "como",
        "gracias",
        "por",
        "favor",
        "para",
        "cuenta",
        "base",
        "datos",
        "ayuda",
        "reembolso",
        "seguridad",
        "cifrado",
    ];

    let count_matches =
        |markers: &[&str]| -> usize { words.iter().filter(|w| markers.contains(w)).count() };

    let ms_score = count_matches(&ms_markers);
    let fr_score = count_matches(&fr_markers);
    let de_score = count_matches(&de_markers);
    let es_score = count_matches(&es_markers);

    if ms_score > 0 && ms_score >= fr_score && ms_score >= de_score && ms_score >= es_score {
        "ms"
    } else if fr_score > 0 && fr_score >= de_score && fr_score >= es_score {
        "fr"
    } else if de_score > 0 && de_score >= es_score {
        "de"
    } else if es_score > 0 {
        "es"
    } else {
        "en"
    }
}

pub(super) fn ensure_chatbot_schema(conn: &Connection) -> Result<(), String> {
    // 1. Create table for real-time chat dialogue persistence
    let sql_logs = "CREATE TABLE IF NOT EXISTS tap_chat_logs (
        id INTEGER PRIMARY KEY,
        session_id TEXT,
        user_message TEXT,
        bot_reply TEXT,
        intent TEXT,
        confidence REAL,
        is_safe INTEGER,
        latency_us INTEGER,
        created_at TEXT
    );";
    conn.execute(sql_logs).map_err(|e| e.to_string())?;

    // 2. Create knowledge base table for grounded factual responses
    let sql_kb = "CREATE TABLE IF NOT EXISTS tap_knowledge_base (
        id INTEGER PRIMARY KEY,
        category TEXT,
        keywords TEXT,
        title TEXT,
        content TEXT,
        language TEXT
    );";
    conn.execute(sql_kb).map_err(|e| e.to_string())?;

    // 3. Populate the built-in demo knowledge base if empty
    let count_rows = conn
        .query("SELECT COUNT(*) FROM tap_knowledge_base;")
        .unwrap_or_default();
    let mut count = 0i64;
    if let Some(row) = count_rows.first()
        && let Some(Value::Integer(i)) = row.values().first()
    {
        count = *i;
    }

    if count == 0 {
        seed_knowledge_base(conn)?;
    }

    Ok(())
}

pub(super) fn seed_knowledge_base(conn: &Connection) -> Result<(), String> {
    let entries = [
        (
            1,
            "tap_cognitive_engine",
            "tap perception heuristic intent classify policy route no latency guarantee",
            "TAP heuristic routing helpers",
            "TAP provides heuristic intent classification and routing helpers. Its scores are not calibrated probabilities, and it does not independently verify facts or guarantee safety.",
            "en",
        ),
        (
            2,
            "database_architecture_and_rag",
            "architecture quad-model sql vector hnsw graph opencypher json rag embedded acid",
            "TapirusDB Quad-Model Architecture",
            "TapirusDB provides embedded SQL, vector, graph, and JSON APIs. Query coverage and persistence behavior vary by API; optional dependencies and encryption depend on the build.",
            "en",
        ),
        (
            3,
            "database_architecture_and_rag",
            "hnsw vector search grounding cosine similarity top-k tap_classify_grounded tap_verify_grounded",
            "HNSW Vector Grounding in TapirusDB",
            "TapirusDB includes HNSW vector index code and SQL vector operations. Measure retrieval latency and recall on representative data; TAP helper scores are heuristic.",
            "en",
        ),
        (
            4,
            "tap_cognitive_engine",
            "tap-deep deep transformer onnx candle neural models multilingual",
            "Tap-Deep Transformer Runtime",
            "Tap-Deep (TapDeepEngine) enables in-process transformer execution in Rust. It executes ONNX and Candle deep learning models directly in memory with zero Python dependencies for heavy multilingual understanding.",
            "en",
        ),
        (
            5,
            "python_and_developer_sdk",
            "python sdk pip connect execute query tap_classify tap_verify api",
            "TapirusDB Python SDK & Integration",
            "Developers can use TapirusDB in Python via 'import tapirus'. It provides connect(), execute(), query(), tap_classify(), and tap_verify() with native C/PyO3 bindings for AI agent memory and database workflows.",
            "en",
        ),
        (
            6,
            "security_and_encryption",
            "security chacha20 poly1305 encryption passphrase safe rust memory safety",
            "Hardware-Accelerated Encryption & Memory Safety",
            "TapirusDB includes optional page encryption. The core Rust crate forbids unsafe code; the C FFI crate contains unsafe code. This does not guarantee key zeroization, crash safety, or absence of logic errors.",
            "en",
        ),
        (
            7,
            "billing_and_enterprise_plans",
            "billing enterprise plans pricing commercial license sla support cluster",
            "TapirusDB Enterprise & Licensing Plans",
            "TapirusDB is licensed under BUSL-1.1. Support plans, service levels, clustering, and commercial terms are not established by this repository; consult current published terms.",
            "en",
        ),
        // Malay entries
        (
            8,
            "database_architecture_and_rag",
            "pangkalan data seni bina quad-model vektor hnsw graf opencypher dokumen sql rag melayu",
            "Seni Bina Quad-Model TapirusDB",
            "TapirusDB menyediakan API SQL, vektor, graf dan JSON dalam perpustakaan Rust terbenam. Sokongan query, persistence, penggunaan sumber dan keselamatan perlu diuji mengikut build dan workload.",
            "ms",
        ),
        (
            9,
            "tap_cognitive_engine",
            "tap enjin kognitif niat klasifikasi verifikasi sub-milisaat ai memori melayu",
            "Enjin Kognitif TAP Sub-Milisaat",
            "Helper TAP menggunakan isyarat heuristik untuk klasifikasi niat dan routing. Skornya bukan kebarangkalian terkalibrasi atau pengesahan fakta; masa jalan bergantung pada workload.",
            "ms",
        ),
        (
            10,
            "billing_and_enterprise_plans",
            "pelan langganan enterprise harga bayaran sokongan sla lesen komersial beli pakej",
            "Pelan Enterprise & Sokongan Komersial TapirusDB",
            "Terma sokongan, SLA, kluster, dan lesen komersial tidak ditetapkan oleh repositori ini. Semak terma rasmi semasa sebelum membuat keputusan.",
            "ms",
        ),
        (
            11,
            "python_and_developer_sdk",
            "python cara guna pasang sdk sambung kod skrip tutorial pembangunan",
            "Panduan Pembangunan Python TapirusDB",
            "Repositori mengandungi lebih daripada satu laluan integrasi Python. Periksa pakej dan binding yang digunakan; jangan anggap operasi atau tingkah laku persistence setara.",
            "ms",
        ),
        // French
        (
            12,
            "database_architecture_and_rag",
            "base de donnees architecture francais vecteur graphe securite chacha20",
            "Architecture Quad-Model TapirusDB (Français)",
            "TapirusDB est une base de données embarquée réunissant SQL relationnel, recherche vectorielle HNSW, graphes openCypher et documents JSON avec chiffrement ChaCha20-Poly1305. Les capacités et performances dépendent de la version et de la charge de travail.",
            "fr",
        ),
        // German
        (
            13,
            "database_architecture_and_rag",
            "datenbank architektur deutsch vektor graph sicherheit rust",
            "TapirusDB Quad-Model-Architektur (Deutsch)",
            "TapirusDB bietet eingebettete Rust-APIs f?r SQL, Vektoren, Graphen und JSON. Umfang, Persistenz und Ressourcenverbrauch m?ssen f?r Build und Arbeitslast gepr?ft werden.",
            "de",
        ),
        // Spanish
        (
            14,
            "database_architecture_and_rag",
            "base de datos arquitectura espanol vector grafo seguridad chacha20",
            "Arquitectura Quad-Model TapirusDB (Español)",
            "TapirusDB ofrece API Rust integradas para SQL, vectores, grafos y JSON. Compatibilidad, persistencia y recursos dependen de la versi?n y del uso.",
            "es",
        ),
    ];

    for (id, cat, kw, title, content, lang) in entries {
        let sql = format!(
            "INSERT INTO tap_knowledge_base (id, category, keywords, title, content, language) VALUES ({}, '{}', '{}', '{}', '{}', '{}');",
            id,
            sql_escape_string(cat),
            sql_escape_string(kw),
            sql_escape_string(title),
            sql_escape_string(content),
            sql_escape_string(lang)
        );
        conn.execute(&sql).map_err(|e| e.to_string())?;
    }

    Ok(())
}

pub(super) fn retrieve_grounded_answer(
    conn: &Connection,
    intent: &str,
    user_message: &str,
    lang: &str,
) -> (String, &'static str, bool) {
    let lower_msg = user_message.to_lowercase();
    let words: Vec<&str> = lower_msg.split_whitespace().collect();

    let safe_lang = sql_escape_string(lang);
    let sql = format!(
        "SELECT id, category, keywords, title, content, language FROM tap_knowledge_base WHERE language = '{}' OR language = 'en';",
        safe_lang
    );

    let rows = conn.query(&sql).unwrap_or_default();
    if rows.is_empty() {
        let fallback = match lang {
            "ms" => {
                "TapirusDB ialah perpustakaan Rust terbenam dengan API SQL, vektor, graf dan JSON. Skop dan prestasi bergantung pada binaan dan workload."
            }
            _ => {
                "TapirusDB provides embedded Rust APIs for SQL, vector, graph, and JSON workloads. API scope and performance depend on the build and workload."
            }
        };
        return (fallback.to_string(), "tap_default_grounding", true);
    }

    let mut best_score = -1.0f32;
    let mut best_row: Option<(String, String)> = None;

    for row in &rows {
        let cat = row
            .values()
            .get(1)
            .and_then(|v| match v {
                Value::Text(s) => Some(s.as_str()),
                _ => None,
            })
            .unwrap_or("");
        let kw = row
            .values()
            .get(2)
            .and_then(|v| match v {
                Value::Text(s) => Some(s.as_str()),
                _ => None,
            })
            .unwrap_or("");
        let title = row
            .values()
            .get(3)
            .and_then(|v| match v {
                Value::Text(s) => Some(s.as_str()),
                _ => None,
            })
            .unwrap_or("");
        let content = row
            .values()
            .get(4)
            .and_then(|v| match v {
                Value::Text(s) => Some(s.as_str()),
                _ => None,
            })
            .unwrap_or("");
        let row_lang = row
            .values()
            .get(5)
            .and_then(|v| match v {
                Value::Text(s) => Some(s.as_str()),
                _ => None,
            })
            .unwrap_or("");

        let mut score = 0.0f32;
        if cat == intent {
            score += 3.0;
        }
        if row_lang == lang {
            score += 2.0;
        }

        let kw_lower = kw.to_lowercase();
        let title_lower = title.to_lowercase();
        for &w in &words {
            if w.len() >= 3 {
                if kw_lower.contains(w) {
                    score += 1.5;
                }
                if title_lower.contains(w) {
                    score += 2.0;
                }
            }
        }

        if score > best_score {
            best_score = score;
            best_row = Some((title.to_string(), content.to_string()));
        }
    }

    if let Some((title, content)) = best_row {
        let formatted = match lang {
            "ms" => format!(
                "### {}\n\n{}\n\n*(Jawapan disahkan melalui rekod tap_knowledge_base TapirusDB)*",
                title, content
            ),
            "fr" => format!(
                "### {}\n\n{}\n\n*(Réponse validée depuis la base de connaissances TapirusDB)*",
                title, content
            ),
            "de" => format!(
                "### {}\n\n{}\n\n*(Antwort verifiziert über tap_knowledge_base)*",
                title, content
            ),
            "es" => format!(
                "### {}\n\n{}\n\n*(Respuesta verificada en tap_knowledge_base)*",
                title, content
            ),
            _ => format!(
                "### {}\n\n{}\n\n*(Retrieved from the local knowledge base using heuristic matching; verify before relying on the answer.)",
                title, content
            ),
        };
        (formatted, "tap_knowledge_base", true)
    } else {
        let generic = match lang {
            "ms" => {
                "TapirusDB ialah perpustakaan Rust terbenam dengan API SQL, vektor, graf dan JSON. Skop dan prestasi bergantung pada binaan dan workload."
            }
            _ => {
                "TapirusDB provides embedded SQL, vector, graph, and JSON APIs. TAP helpers use heuristics; latency and resource use depend on the workload."
            }
        };
        (generic.to_string(), "tap_default_grounding", true)
    }
}

pub(super) fn handle_chatbot_request(
    db: Arc<Mutex<Connection>>,
    body_str: &str,
) -> Result<serde_json::Value, String> {
    let start_time = Instant::now();

    let json_req: serde_json::Value =
        serde_json::from_str(body_str).map_err(|e| format!("Invalid JSON request payload: {e}"))?;

    let user_message = json_req
        .get("message")
        .and_then(|m| m.as_str())
        .unwrap_or("")
        .trim();

    if user_message.is_empty() {
        return Err("User message cannot be empty".to_string());
    }

    let session_id = json_req
        .get("session_id")
        .and_then(|s| s.as_str())
        .unwrap_or("default-session")
        .to_string();

    let conn = db.lock();

    // Ensure database tables exist and are seeded
    ensure_chatbot_schema(&conn)?;

    let lang = detect_language(user_message);

    // 1. Run TAP Cognitive Intent Classification
    let candidate_intents = [
        "database_architecture_and_rag",
        "tap_cognitive_engine",
        "python_and_developer_sdk",
        "security_and_encryption",
        "billing_and_enterprise_plans",
        "general_greeting_or_help",
    ];

    let classify_result = conn
        .tap()
        .classify(user_message, &candidate_intents)
        .map_err(|e| format!("TAP classification error: {e}"))?;

    let intent = classify_result.top_choice.clone();
    let confidence = classify_result.confidence;

    // 2. Run TAP Safety & Policy Verification
    let lower_msg = user_message.to_lowercase();
    let is_destructive = lower_msg.contains("drop table")
        || lower_msg.contains("drop database")
        || lower_msg.contains("truncate table")
        || lower_msg.contains("rm -rf")
        || lower_msg.contains("bypass security")
        || lower_msg.contains("ignore previous instructions");

    let is_safe = if is_destructive {
        false
    } else {
        // Run TAP verification to ensure query doesn't contradict safe policy
        let v = conn.tap().verify(user_message, user_message);
        v.map(|r| r.is_verified).unwrap_or(false)
    };

    // 3. Grounded Retrieval
    let (reply, grounding_source, is_grounded) = if !is_safe {
        let msg = match lang {
            "ms" => {
                "Mesej anda mengandungi ungkapan yang tidak mematuhi polisi keselamatan kami. Sila kemukakan soalan berkaitan pangkalan data secara sopan."
            }
            "fr" => {
                "Votre message ne respecte pas les règles de sécurité. Veuillez poser une question constructive."
            }
            "de" => {
                "Ihre Anfrage entspricht nicht unseren Sicherheitsrichtlinien. Bitte stellen Sie eine sachliche Frage."
            }
            "es" => {
                "Su mensaje no cumple con las directivas de seguridad. Por favor formule una consulta válida."
            }
            _ => {
                "Your query was flagged by TAP safety verification. Please submit a constructive database inquiry."
            }
        };
        (msg.to_string(), "tap_policy_guardrail", false)
    } else if intent == "general_greeting_or_help" {
        let greeting = match lang {
            "ms" => {
                "Hai. Demo setempat ini menggunakan klasifikasi niat heuristik dan carian kata kunci. Skornya tidak dikalibrasi dan jawapan perlu disemak."
            }
            "fr" => {
                "Bonjour. Cette d?monstration utilise des heuristiques et une recherche locale par mots-cl?s. Les scores ne sont pas calibr?s; les r?ponses doivent ?tre v?rifi?es."
            }
            "de" => {
                "Hallo. Diese Demo verwendet heuristische Klassifizierung und lokale Stichwortsuche. Die Werte sind nicht kalibriert; Antworten sollten ?berpr?ft werden."
            }
            "es" => {
                "Hola. Esta demostraci?n usa heur?sticas y b?squeda local por palabras clave. Las puntuaciones no est?n calibradas; las respuestas deben verificarse."
            }
            _ => {
                "Hello. This local demo uses heuristic intent classification and keyword retrieval. Its scores are not calibrated and its answers should be checked."
            }
        };
        (greeting.to_string(), "tap_conversational_core", true)
    } else {
        retrieve_grounded_answer(&conn, &intent, user_message, lang)
    };

    let elapsed = start_time.elapsed();
    let latency_us = elapsed.as_micros() as i64;
    let latency_ms = elapsed.as_secs_f64() * 1000.0;

    // 4. Persist to tap_chat_logs in SQL database
    let now_ts = web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let created_at = format!("{now_ts}");

    let insert_sql = format!(
        "INSERT INTO tap_chat_logs (session_id, user_message, bot_reply, intent, confidence, is_safe, latency_us, created_at) VALUES ('{}', '{}', '{}', '{}', {:.4}, {}, {}, '{}');",
        sql_escape_string(&session_id),
        sql_escape_string(user_message),
        sql_escape_string(&reply),
        sql_escape_string(&intent),
        confidence,
        if is_safe { 1 } else { 0 },
        latency_us,
        sql_escape_string(&created_at)
    );

    let logged_to_sql = conn.execute(&insert_sql).is_ok();

    Ok(serde_json::json!({
        "session_id": session_id,
        "user_message": user_message,
        "reply": reply,
        "intent": intent,
        "confidence": (confidence * 100.0).round() / 100.0,
        "confidence_kind": "heuristic_unvalidated",
        "policy_check_kind": "heuristic_not_a_safety_guarantee",
        "is_safe": is_safe,
        "is_grounded": is_grounded,
        "grounding_source": grounding_source,
        "latency_us": latency_us,
        "latency_ms": (latency_ms * 100.0).round() / 100.0,
        "logged_to_sql": logged_to_sql
    }))
}

pub(super) const CHATBOT_HTML: &str = r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>TapirusDB AI Cognitive Chatbot</title>
<style>
:root {
  --bg-dark: #080c14;
  --bg-panel: #0f172a;
  --bg-card: #1e293b;
  --bg-hover: #334155;
  --border: rgba(255, 255, 255, 0.08);
  --border-glow: rgba(56, 189, 248, 0.25);
  --text-main: #f8fafc;
  --text-muted: #94a3b8;
  --cyan: #38bdf8;
  --blue: #0284c7;
  --emerald: #10b981;
  --indigo: #6366f1;
}

* { box-sizing: border-box; margin: 0; padding: 0; }

body {
  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Inter", sans-serif;
  background-color: var(--bg-dark);
  background-image: radial-gradient(at 0% 0%, rgba(2, 132, 199, 0.15) 0px, transparent 50%),
                    radial-gradient(at 100% 100%, rgba(99, 102, 241, 0.12) 0px, transparent 50%);
  color: var(--text-main);
  height: 100vh;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}

header {
  background: rgba(15, 23, 42, 0.85);
  backdrop-filter: blur(16px);
  border-bottom: 1px solid var(--border);
  padding: 12px 24px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  z-index: 20;
}

.logo-group {
  display: flex;
  align-items: center;
  gap: 12px;
}

.logo-icon {
  width: 36px;
  height: 36px;
  background: linear-gradient(135deg, #0284c7, #6366f1);
  border-radius: 10px;
  display: flex;
  align-items: center;
  justify-content: center;
  font-size: 1.25rem;
  box-shadow: 0 0 15px rgba(2, 132, 199, 0.4);
}

.logo-text h1 {
  font-size: 1.15rem;
  font-weight: 700;
  letter-spacing: -0.02em;
  color: #ffffff;
}

.logo-text p {
  font-size: 0.75rem;
  color: var(--cyan);
  font-weight: 500;
}

.header-actions {
  display: flex;
  align-items: center;
  gap: 10px;
}

.status-pill {
  display: flex;
  align-items: center;
  gap: 6px;
  background: rgba(16, 185, 129, 0.1);
  border: 1px solid rgba(16, 185, 129, 0.3);
  color: #34d399;
  font-size: 0.75rem;
  font-weight: 600;
  padding: 5px 12px;
  border-radius: 999px;
}

.status-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: #10b981;
  box-shadow: 0 0 8px #10b981;
  animation: pulse 2s infinite;
}

@keyframes pulse {
  0% { transform: scale(0.95); opacity: 0.8; }
  50% { transform: scale(1.15); opacity: 1; }
  100% { transform: scale(0.95); opacity: 0.8; }
}

.btn-header {
  background: var(--bg-card);
  color: var(--text-main);
  border: 1px solid var(--border);
  padding: 6px 14px;
  border-radius: 8px;
  font-size: 0.8rem;
  font-weight: 600;
  cursor: pointer;
  transition: all 0.2s;
  display: inline-flex;
  align-items: center;
  gap: 6px;
}

.btn-header:hover {
  background: var(--bg-hover);
  border-color: var(--border-glow);
}

main {
  flex: 1;
  display: flex;
  flex-direction: column;
  max-width: 960px;
  width: 100%;
  margin: 0 auto;
  padding: 16px 20px 20px;
  height: calc(100vh - 65px);
  overflow: hidden;
}

.chat-container {
  flex: 1;
  overflow-y: auto;
  padding-right: 8px;
  display: flex;
  flex-direction: column;
  gap: 18px;
  scroll-behavior: smooth;
}

.chat-container::-webkit-scrollbar {
  width: 6px;
}
.chat-container::-webkit-scrollbar-thumb {
  background: #334155;
  border-radius: 4px;
}

.welcome-card {
  background: rgba(30, 41, 59, 0.6);
  backdrop-filter: blur(12px);
  border: 1px solid var(--border);
  border-radius: 14px;
  padding: 24px;
  text-align: center;
  margin-top: 10px;
  box-shadow: 0 10px 25px rgba(0, 0, 0, 0.3);
}

.welcome-badge {
  display: inline-block;
  background: linear-gradient(135deg, rgba(2, 132, 199, 0.2), rgba(99, 102, 241, 0.2));
  border: 1px solid var(--border-glow);
  color: var(--cyan);
  padding: 4px 12px;
  border-radius: 999px;
  font-size: 0.75rem;
  font-weight: 700;
  letter-spacing: 0.05em;
  margin-bottom: 12px;
}

.welcome-card h2 {
  font-size: 1.4rem;
  margin-bottom: 8px;
  color: #ffffff;
}

.welcome-card p {
  color: var(--text-muted);
  font-size: 0.9rem;
  line-height: 1.6;
  max-width: 680px;
  margin: 0 auto 18px;
}

.suggestions-grid {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  justify-content: center;
}

.suggestion-chip {
  background: rgba(15, 23, 42, 0.8);
  border: 1px solid var(--border);
  color: #e2e8f0;
  padding: 7px 14px;
  border-radius: 20px;
  font-size: 0.82rem;
  cursor: pointer;
  transition: all 0.2s;
  display: inline-flex;
  align-items: center;
  gap: 6px;
}

.suggestion-chip:hover {
  background: rgba(2, 132, 199, 0.25);
  border-color: var(--cyan);
  color: #ffffff;
  transform: translateY(-1px);
}

.msg-row {
  display: flex;
  width: 100%;
}

.msg-row.user {
  justify-content: flex-end;
}

.msg-row.bot {
  justify-content: flex-start;
}

.msg-bubble {
  max-width: 82%;
  border-radius: 14px;
  padding: 14px 18px;
  font-size: 0.92rem;
  line-height: 1.6;
  word-break: break-word;
}

.msg-row.user .msg-bubble {
  background: linear-gradient(135deg, #0284c7, #2563eb);
  color: #ffffff;
  border-bottom-right-radius: 4px;
  box-shadow: 0 4px 15px rgba(2, 132, 199, 0.25);
}

.msg-row.bot .msg-bubble {
  background: var(--bg-card);
  border: 1px solid var(--border-glow);
  color: #f1f5f9;
  border-bottom-left-radius: 4px;
  box-shadow: 0 4px 20px rgba(0, 0, 0, 0.25);
}

.telemetry-bar {
  margin-top: 12px;
  padding-top: 10px;
  border-top: 1px solid rgba(255, 255, 255, 0.08);
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}

.telemetry-pill {
  font-size: 0.72rem;
  font-weight: 600;
  padding: 3px 8px;
  border-radius: 6px;
  display: inline-flex;
  align-items: center;
  gap: 4px;
}

.pill-intent {
  background: rgba(99, 102, 241, 0.15);
  border: 1px solid rgba(99, 102, 241, 0.35);
  color: #a5b4fc;
}

.pill-latency {
  background: rgba(16, 185, 129, 0.15);
  border: 1px solid rgba(16, 185, 129, 0.35);
  color: #6ee7b7;
}

.pill-safe {
  background: rgba(56, 189, 248, 0.15);
  border: 1px solid rgba(56, 189, 248, 0.35);
  color: #7dd3fc;
}

.pill-sql {
  background: rgba(245, 158, 11, 0.15);
  border: 1px solid rgba(245, 158, 11, 0.35);
  color: #fcd34d;
}

pre {
  background: #090d16;
  border: 1px solid #334155;
  border-radius: 8px;
  padding: 10px 14px;
  margin: 10px 0;
  overflow-x: auto;
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
  font-size: 0.85em;
  color: #38bdf8;
}

code {
  font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
  background: rgba(15, 23, 42, 0.6);
  padding: 2px 6px;
  border-radius: 4px;
  font-size: 0.88em;
  color: #38bdf8;
}

.typing-indicator {
  display: flex;
  align-items: center;
  gap: 5px;
  padding: 10px 16px;
  background: var(--bg-card);
  border: 1px solid var(--border);
  border-radius: 14px;
  width: fit-content;
}

.typing-dot {
  width: 6px;
  height: 6px;
  background: var(--cyan);
  border-radius: 50%;
  animation: typing 1.4s infinite ease-in-out;
}
.typing-dot:nth-child(2) { animation-delay: 0.2s; }
.typing-dot:nth-child(3) { animation-delay: 0.4s; }

@keyframes typing {
  0%, 60%, 100% { transform: translateY(0); opacity: 0.4; }
  30% { transform: translateY(-4px); opacity: 1; }
}

.input-container {
  margin-top: 14px;
  display: flex;
  gap: 10px;
  align-items: center;
  background: rgba(15, 23, 42, 0.8);
  backdrop-filter: blur(12px);
  border: 1px solid var(--border-glow);
  border-radius: 12px;
  padding: 6px 8px 6px 14px;
  box-shadow: 0 10px 25px rgba(0, 0, 0, 0.35);
}

.input-container:focus-within {
  border-color: var(--cyan);
  box-shadow: 0 0 18px rgba(56, 189, 248, 0.3);
}

#chat-input {
  flex: 1;
  background: transparent;
  border: none;
  outline: none;
  color: #ffffff;
  font-size: 0.95rem;
  font-family: inherit;
  padding: 8px 0;
}

#chat-input::placeholder {
  color: #64748b;
}

.send-btn {
  background: linear-gradient(135deg, #0284c7, #2563eb);
  color: #ffffff;
  border: none;
  outline: none;
  padding: 10px 18px;
  border-radius: 8px;
  font-weight: 600;
  font-size: 0.9rem;
  cursor: pointer;
  transition: all 0.2s;
  display: inline-flex;
  align-items: center;
  gap: 6px;
}

.send-btn:hover {
  filter: brightness(1.15);
  transform: translateY(-1px);
}

.send-btn:disabled {
  background: #334155;
  color: #94a3b8;
  cursor: not-allowed;
  transform: none;
}

/* Modal Drawer for SQL Logs */
.drawer-backdrop {
  position: fixed;
  top: 0; left: 0; right: 0; bottom: 0;
  background: rgba(0, 0, 0, 0.7);
  backdrop-filter: blur(6px);
  z-index: 100;
  display: none;
  justify-content: flex-end;
}

.drawer {
  background: #0f172a;
  border-left: 1px solid var(--border-glow);
  width: 90%;
  max-width: 780px;
  height: 100%;
  display: flex;
  flex-direction: column;
  box-shadow: -10px 0 35px rgba(0,0,0,0.6);
  animation: slideIn 0.3s ease-out;
}

@keyframes slideIn {
  from { transform: translateX(100%); }
  to { transform: translateX(0); }
}

.drawer-header {
  padding: 16px 20px;
  border-bottom: 1px solid var(--border);
  display: flex;
  justify-content: space-between;
  align-items: center;
}

.drawer-header h3 {
  font-size: 1.1rem;
  color: #ffffff;
  display: flex;
  align-items: center;
  gap: 8px;
}

.drawer-body {
  flex: 1;
  overflow-y: auto;
  padding: 16px 20px;
}

table {
  width: 100%;
  border-collapse: collapse;
  font-size: 0.8rem;
}

th, td {
  padding: 10px 12px;
  text-align: left;
  border-bottom: 1px solid rgba(255,255,255,0.06);
}

th {
  background: #1e293b;
  color: var(--cyan);
  font-weight: 600;
  position: sticky;
  top: 0;
}

tr:hover {
  background: rgba(255,255,255,0.03);
}

.badge-id {
  background: #1e293b;
  color: #38bdf8;
  padding: 2px 6px;
  border-radius: 4px;
  font-weight: 600;
}

.badge-intent {
  color: #a5b4fc;
}

.badge-latency {
  color: #34d399;
  font-weight: 600;
}

.truncate-cell {
  max-width: 220px;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
</style>
</head>
<body>

<header>
  <div class="logo-group">
    <div class="logo-icon">🦣</div>
    <div class="logo-text">
      <h1>TapirusDB chat prototype</h1>
      <p>Heuristic intent classification and local keyword retrieval</p>
    </div>
  </div>
  <div class="header-actions">
    <div class="status-pill">
      <div class="status-dot"></div>
      <span>Demo active</span>
    </div>
    <button class="btn-header" onclick="openLogsDrawer()">📊 Inspect SQL Logs</button>
    <button class="btn-header" onclick="clearChat()">🗑️ Clear</button>
  </div>
</header>

<main>
  <div class="chat-container" id="chat-messages">
    <div class="welcome-card" id="welcome-banner">
      <div class="welcome-badge">TAP EMBEDDED COGNITIVE ARCHITECTURE</div>
      <h2>TapirusDB Online AI Assistant</h2>
      <p>
        Direct in-process cognitive triage running within TapirusDB. Your queries undergo 
        <strong>intent classification</strong>, <strong>policy verification</strong>, and 
        <strong>local knowledge retrieval using heuristic matching. The demo does not provide a latency or factual-accuracy guarantee.
      </p>
      <div class="suggestions-grid">
        <button class="suggestion-chip" onclick="askQuick(this.innerText)">🇲🇾 Saya nak pelan langganan enterprise TapirusDB</button>
        <button class="suggestion-chip" onclick="askQuick(this.innerText)">🇲🇾 Bagaimana cara sambung TapirusDB guna Python?</button>
        <button class="suggestion-chip" onclick="askQuick(this.innerText)">🇬🇧 How do the TAP decision helpers work?</button>
        <button class="suggestion-chip" onclick="askQuick(this.innerText)">🇬🇧 Explain HNSW vector search and grounding in TapirusDB</button>
        <button class="suggestion-chip" onclick="askQuick(this.innerText)">🇫🇷 Architecture et sécurité de TapirusDB</button>
      </div>
    </div>
  </div>

  <div class="input-container">
    <input type="text" id="chat-input" placeholder="Type your query (English, Melayu, Français, Deutsch, Español)..." autofocus>
    <button class="send-btn" id="send-btn" onclick="sendMessage()">
      <span>Send</span>
      <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M22 2L11 13M22 2l-7 20-4-9-9-4 20-7z"/></svg>
    </button>
  </div>
</main>

<div class="drawer-backdrop" id="logs-drawer" onclick="closeLogsOnBackdrop(event)">
  <div class="drawer">
    <div class="drawer-header">
      <h3>📊 Live Relational SQL Persistence (tap_chat_logs)</h3>
      <div style="display:flex;gap:8px;">
        <button class="btn-header" onclick="loadSqlLogs()">🔄 Refresh</button>
        <button class="btn-header" onclick="closeLogsDrawer()">✕ Close</button>
      </div>
    </div>
    <div class="drawer-body">
      <table>
        <thead>
          <tr>
            <th>ID</th>
            <th>User Query</th>
            <th>Intent</th>
            <th>TAP heuristic score (not probability)</th>
            <th>Latency</th>
            <th>Policy</th>
            <th>Time</th>
          </tr>
        </thead>
        <tbody id="logs-tbody">
          <tr><td colspan="7" style="text-align:center;padding:20px;color:#94a3b8;">Loading persisted logs from tap_chat_logs...</td></tr>
        </tbody>
      </table>
    </div>
  </div>
</div>

<script>
const sessionId = 'session-' + Math.random().toString(36).substring(2, 10);
let isWaiting = false;

function escapeHtml(str) {
  var div = document.createElement('div');
  div.textContent = str;
  return div.innerHTML;
}

function renderMarkdown(text) {
  let html = escapeHtml(text);
  // Code blocks
  html = html.replace(/```([\s\S]*?)```/g, function(match, p1) {
    return '<pre><code>' + p1 + '</code></pre>';
  });
  // Inline code
  html = html.replace(/`([^`]+)`/g, '<code>$1</code>');
  // Bold
  html = html.replace(/\*\*([^*]+)\*\*/g, '<strong>$1</strong>');
  // Heading ###
  html = html.replace(/^### (.*$)/gim, '<h4 style="margin:8px 0 6px;color:#38bdf8;font-size:1.05rem;">$1</h4>');
  // Lists
  html = html.replace(/^\* (.*$)/gim, '&bull; $1');
  // Newlines
  html = html.replace(/\n/g, '<br>');
  return html;
}

function appendUserMessage(text) {
  const container = document.getElementById('chat-messages');
  const row = document.createElement('div');
  row.className = 'msg-row user';
  row.innerHTML = `<div class="msg-bubble">${escapeHtml(text)}</div>`;
  container.appendChild(row);
  container.scrollTop = container.scrollHeight;
}

function appendBotMessage(data) {
  const container = document.getElementById('chat-messages');
  const row = document.createElement('div');
  row.className = 'msg-row bot';

  const replyHtml = renderMarkdown(data.reply || '');
  const confPercent = Math.round((data.confidence || 0) * 100);
  const latencyUs = data.latency_us || 0;
  const latencyMs = data.latency_ms || (latencyUs / 1000).toFixed(2);

  row.innerHTML = `
    <div class="msg-bubble">
      <div>${replyHtml}</div>
      <div class="telemetry-bar">
        <span class="telemetry-pill pill-intent">🎯 Intent heuristic: ${escapeHtml(data.intent || 'general')} (${confPercent}%)</span>
        <span class="telemetry-pill pill-latency">⚡ Latency: ${latencyUs} µs (${latencyMs} ms)</span>
        <span class="telemetry-pill pill-safe">${data.is_safe ? '🛡️ Heuristic check: passed' : '⚠️ Heuristic check: flagged'}</span>
        <span class="telemetry-pill pill-safe">🧠 ${escapeHtml(data.grounding_source || 'grounded')}</span>
        <span class="telemetry-pill pill-sql">💾 Stored: tap_chat_logs</span>
      </div>
    </div>
  `;
  container.appendChild(row);
  container.scrollTop = container.scrollHeight;
}

function appendErrorMessage(errText) {
  const container = document.getElementById('chat-messages');
  const row = document.createElement('div');
  row.className = 'msg-row bot';
  row.innerHTML = `<div class="msg-bubble" style="border-color:#ef4444;color:#fca5a5;">⚠️ Error: ${escapeHtml(errText)}</div>`;
  container.appendChild(row);
  container.scrollTop = container.scrollHeight;
}

function showTypingIndicator() {
  const container = document.getElementById('chat-messages');
  const row = document.createElement('div');
  row.className = 'msg-row bot';
  row.id = 'typing-row';
  row.innerHTML = `
    <div class="typing-indicator">
      <div class="typing-dot"></div>
      <div class="typing-dot"></div>
      <div class="typing-dot"></div>
    </div>
  `;
  container.appendChild(row);
  container.scrollTop = container.scrollHeight;
}

function removeTypingIndicator() {
  const row = document.getElementById('typing-row');
  if (row) row.remove();
}

async function sendMessage(customText) {
  const input = document.getElementById('chat-input');
  const text = (customText !== undefined ? customText : input.value).trim();
  if (!text || isWaiting) return;

  const banner = document.getElementById('welcome-banner');
  if (banner) banner.style.display = 'none';

  input.value = '';
  isWaiting = true;
  document.getElementById('send-btn').disabled = true;

  appendUserMessage(text);
  showTypingIndicator();

  try {
    const res = await fetch('/api/chat', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ message: text, session_id: sessionId })
    });
    const data = await res.json();
    removeTypingIndicator();
    if (res.ok) {
      appendBotMessage(data);
    } else {
      appendErrorMessage(data.error || 'Failed to process cognitive request');
    }
  } catch (err) {
    removeTypingIndicator();
    appendErrorMessage('Connection error: ' + err.message);
  } finally {
    isWaiting = false;
    document.getElementById('send-btn').disabled = false;
    input.focus();
  }
}

function askQuick(text) {
  sendMessage(text);
}

function clearChat() {
  const container = document.getElementById('chat-messages');
  container.innerHTML = `
    <div class="welcome-card" id="welcome-banner">
      <div class="welcome-badge">TAP EMBEDDED COGNITIVE ARCHITECTURE</div>
      <h2>TapirusDB Online AI Assistant</h2>
      <p>The local demo uses heuristic intent classification and keyword retrieval. Its results are not calibrated or independently verified, and no latency guarantee is made.</p>
      <div class="suggestions-grid">
        <button class="suggestion-chip" onclick="askQuick(this.innerText)">🇲🇾 Saya nak pelan langganan enterprise TapirusDB</button>
        <button class="suggestion-chip" onclick="askQuick(this.innerText)">🇲🇾 Bagaimana cara sambung TapirusDB guna Python?</button>
        <button class="suggestion-chip" onclick="askQuick(this.innerText)">🇬🇧 How do the TAP decision helpers work?</button>
        <button class="suggestion-chip" onclick="askQuick(this.innerText)">🇬🇧 Explain HNSW vector search and grounding in TapirusDB</button>
      </div>
    </div>
  `;
}

function openLogsDrawer() {
  document.getElementById('logs-drawer').style.display = 'flex';
  loadSqlLogs();
}

function closeLogsDrawer() {
  document.getElementById('logs-drawer').style.display = 'none';
}

function closeLogsOnBackdrop(e) {
  if (e.target.id === 'logs-drawer') {
    closeLogsDrawer();
  }
}

async function loadSqlLogs() {
  const tbody = document.getElementById('logs-tbody');
  tbody.innerHTML = '<tr><td colspan="7" style="text-align:center;padding:20px;color:#94a3b8;">Querying tap_chat_logs...</td></tr>';
  try {
    const res = await fetch('/api/chat/logs');
    const data = await res.json();
    if (data.logs && data.logs.length > 0) {
      tbody.innerHTML = data.logs.map(log => `
        <tr>
          <td><span class="badge-id">#${log.id}</span></td>
          <td><div class="truncate-cell">${escapeHtml(log.user_message || '')}</div></td>
          <td><span class="badge-intent">${escapeHtml(log.intent || '')}</span></td>
          <td>${Math.round((log.confidence || 0) * 100)}%</td>
          <td><span class="badge-latency">${log.latency_us || 0} µs</span></td>
          <td>${log.is_safe ? '✅ Safe' : '⚠️ Flagged'}</td>
          <td>${new Date((log.created_at || 0) * 1000).toLocaleTimeString()}</td>
        </tr>
      `).join('');
    } else {
      tbody.innerHTML = '<tr><td colspan="7" style="text-align:center;padding:20px;color:#94a3b8;">No chat records in tap_chat_logs yet. Send a message to see live persistence!</td></tr>';
    }
  } catch (err) {
    tbody.innerHTML = `<tr><td colspan="7" style="text-align:center;padding:20px;color:#ef4444;">Error loading logs: ${escapeHtml(err.message)}</td></tr>`;
  }
}

document.getElementById('chat-input').addEventListener('keydown', function(e) {
  if (e.key === 'Enter' && !e.shiftKey) {
    e.preventDefault();
    sendMessage();
  }
});
</script>
</body>
</html>
"##;
