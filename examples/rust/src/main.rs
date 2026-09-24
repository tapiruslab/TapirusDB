use tapirus::{Connection, Direction, Result};

fn main() -> Result<()> {
    println!("🚀 TapirusDB Native Safe-Rust Quad-Model Example");

    // 1. Open an encrypted database or in-memory connection
    let conn = Connection::open_in_memory()?;

    // 2. Relational SQL & AI Vector Search
    conn.execute("CREATE TABLE drones (id INTEGER PRIMARY KEY, model TEXT, embedding VECTOR(3));")?;
    conn.execute("INSERT INTO drones VALUES (1, 'Valkyrie-X', [0.1, 0.9, 0.1]);")?;
    conn.execute("INSERT INTO drones VALUES (2, 'Specter-9', [0.8, 0.1, 0.0]);")?;

    let rows = conn.query("SELECT id, model FROM drones VECTOR NEAR embedding = [0.12, 0.88, 0.08] TOP 1;")?;
    println!("\n⚡ Nearest Vector Drone Match:");
    for r in rows {
        println!(" - ID: {} | Model: {}", r.get::<i64>("id")?, r.get::<String>("model")?);
    }

    // 3. Schema-less MongoDB-style JSON Documents
    let docs = conn.collection("fleet_telemetry")?;
    docs.insert_one(&serde_json::json!({
        "drone_id": 1,
        "battery_pct": 94.2,
        "mode": "AUTONOMOUS",
        "coordinates": {"lat": 3.1390, "lon": 101.6869}
    }))?;

    let telemetry = docs.find_all()?;
    println!("\n📄 Document Store ({} entries):", telemetry.len());
    println!(" - First doc: {:?}", telemetry[0].1);

    // 4. Native Property Knowledge Graph (GraphRAG)
    conn.graph_add_node(1, "DRONE", "{\"name\": \"Valkyrie-X\"}")?;
    conn.graph_add_node(100, "BASE_STATION", "{\"location\": \"Alpha HQ\"}")?;
    conn.graph_add_edge(1, 100, "COMMUNICATES_WITH", 1.0, "{\"frequency\": \"5.8GHz\"}")?;

    let neighbors = conn.graph_neighbors(1, Direction::Outgoing, Some("COMMUNICATES_WITH"));
    println!("\n🕸️ Knowledge Graph Connections:");
    for (node, edge) in neighbors {
        println!(" - Node {} -> Node {} via {}", node.id, node.label, edge.label);
    }

    println!("\n✓ All Quad-Models operational in pure 100% Safe Rust!");
    Ok(())
}
