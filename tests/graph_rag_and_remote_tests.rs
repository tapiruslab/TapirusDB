use std::sync::Arc;
use tapirus::pager::remote::{MockRemoteRangeStorage, RemotePager, S3StorageConfig};
use tapirus::pager::{DatabaseHeader, DATABASE_HEADER_SIZE};
use tapirus::{Connection, GraphRagConfig, Result};

#[test]
fn test_graph_rag_pq_seed_and_micro_hop_traversal() -> Result<()> {
    let conn = Connection::open_in_memory()?;

    // 1. Ingest Knowledge Graph entities with dense vectors
    conn.graph_add_node_with_vector(
        1,
        "Albert Einstein",
        r#"{"field": "Theoretical Physics", "born": 1879}"#,
        Some(&[0.92, 0.15, 0.05]),
    )?;

    conn.graph_add_node_with_vector(
        2,
        "Theory of Relativity",
        r#"{"type": "Fundamental Physics", "year": 1915}"#,
        Some(&[0.90, 0.18, 0.02]),
    )?;

    conn.graph_add_node_with_vector(
        3,
        "Nobel Prize in Physics",
        r#"{"year": 1921, "award": "Photoelectric Effect"}"#,
        Some(&[0.20, 0.85, 0.10]),
    )?;

    conn.graph_add_node_with_vector(
        4,
        "Max Planck",
        r#"{"field": "Quantum Mechanics", "constant": "h"}"#,
        Some(&[0.75, 0.30, 0.20]),
    )?;

    // 2. Establish factual relationship edges
    conn.graph_add_edge(1, 2, "FORMULATED", 1.0, "{}")?;
    conn.graph_add_edge(1, 3, "AWARDED", 0.95, "{}")?;
    conn.graph_add_edge(4, 1, "COLLABORATED_WITH", 0.85, "{}")?;

    // 3. Execute accelerated GraphRAG query with vector seeding
    let query_vector = [0.91, 0.16, 0.04];
    let config = GraphRagConfig::default()
        .with_seeds(2)
        .with_max_hops(2)
        .with_limit(4)
        .with_weights(0.5, 0.3, 0.2);

    let rag_context = conn.graph_rag_query("Relativity and Physics", Some(&query_vector), &config)?;

    // Verify seed discovery & retrieval
    assert_eq!(rag_context.query, "Relativity and Physics");
    assert!(!rag_context.results.is_empty(), "GraphRAG should return matched entities");

    // The top seed should be Node 1 or Node 2 (closest to query vector)
    let top_match = &rag_context.results[0];
    assert!(
        top_match.entity_id == 1 || top_match.entity_id == 2,
        "Top entity should be Einstein or Relativity"
    );
    assert_eq!(top_match.hop_distance, 0, "Seed node should have 0 hop distance");

    // Verify micro-hop expansion reached connected entities (hop distance 1)
    let has_connected_hop = rag_context.results.iter().any(|r| r.hop_distance == 1);
    assert!(has_connected_hop, "Micro-hop traversal should discover 1st degree neighbors");

    // Verify prompt synthesis
    assert!(
        rag_context.prompt_context.contains("Verified Knowledge Graph Context"),
        "Context must include header"
    );
    assert!(
        rag_context.prompt_context.contains("Relationships:"),
        "Context must render entity relations"
    );

    Ok(())
}

#[test]
fn test_remote_pager_s3_range_streaming_and_caching() -> Result<()> {
    // 1. Prepare raw database bytes with valid 4KB header
    let page_size = 4096usize;
    let mut db_bytes = vec![0u8; page_size * 2]; // 2 pages

    // Write valid TapirusDB Page 1 Header
    let mut header = DatabaseHeader::new(page_size as u16);
    header.total_pages = 2;
    db_bytes[0..DATABASE_HEADER_SIZE].copy_from_slice(&header.to_bytes());

    // 2. Initialize Mock Remote Storage (simulating S3 / Cloudflare R2)
    let mock_storage = Arc::new(MockRemoteRangeStorage::new(db_bytes));
    assert_eq!(mock_storage.fetch_count(), 0);

    // 3. Open RemotePager
    let mut pager = RemotePager::open(mock_storage.clone(), 64, None)?;
    assert_eq!(pager.total_pages(), 2);
    assert_eq!(pager.page_size(), 4096);

    // Opening fetches header [0..100] -> fetch count is 1
    assert_eq!(mock_storage.fetch_count(), 1);

    // 4. First read of Page 1 (network fetch)
    let p1 = pager.read_page(1)?;
    assert_eq!(p1.len(), 4096);
    assert_eq!(pager.network_fetches(), 1);
    assert_eq!(pager.cache_hits(), 0);

    // 5. Second read of Page 1 (cache hit in memory, zero remote fetch)
    let p1_again = pager.read_page(1)?;
    assert_eq!(p1_again.len(), 4096);
    assert_eq!(pager.network_fetches(), 1); // Still 1 fetch!
    assert_eq!(pager.cache_hits(), 1); // 1 cache hit!

    // 6. Test S3StorageConfig range header generation
    let s3_config = S3StorageConfig {
        bucket: "tapirus-production".into(),
        key: "db/analytics.tapir".into(),
        endpoint: "https://r2.cloudflarestorage.com".into(),
        region: "auto".into(),
        auth_token: None,
    };
    let range_header = s3_config.make_range_header(4096, 4096);
    assert_eq!(range_header, "bytes=4096-8191");

    Ok(())
}

#[test]
fn test_graph_rag_tenant_and_acl_filtering() -> Result<()> {
    let conn = Connection::open_in_memory()?;

    // Ingest entities across two distinct enterprise tenants: "cyberdyne" and "weyland"
    // Node 1: Cyberdyne Project Genesis (Admin only)
    conn.graph_add_node_with_vector(
        1,
        "Project Genesis",
        r#"{"tenant_id": "cyberdyne", "roles": ["admin"], "project": "genesis_ai"}"#,
        Some(&[0.90, 0.10, 0.00]),
    )?;

    // Node 2: Cyberdyne Core Architecture (Admin & Analyst)
    conn.graph_add_node_with_vector(
        2,
        "Genesis Architecture",
        r#"{"tenant_id": "cyberdyne", "roles": ["admin", "analyst"], "project": "genesis_ai"}"#,
        Some(&[0.85, 0.15, 0.00]),
    )?;

    // Node 3: Cyberdyne Public Briefing (Public visibility)
    conn.graph_add_node_with_vector(
        3,
        "Genesis Public Overview",
        r#"{"tenant_id": "cyberdyne", "visibility": "public", "project": "genesis_ai"}"#,
        Some(&[0.80, 0.20, 0.00]),
    )?;

    // Node 4: Weyland Project Prometheus (Admin only, highly similar vector)
    conn.graph_add_node_with_vector(
        4,
        "Project Prometheus",
        r#"{"tenant_id": "weyland", "roles": ["admin"], "mission": "deep_space"}"#,
        Some(&[0.95, 0.05, 0.00]),
    )?;

    // Node 5: Weyland Propulsion (Analyst only)
    conn.graph_add_node_with_vector(
        5,
        "Prometheus Engine",
        r#"{"tenant_id": "weyland", "roles": ["analyst"], "mission": "deep_space"}"#,
        Some(&[0.88, 0.12, 0.00]),
    )?;

    // Intra-tenant and cross-tenant relationships
    conn.graph_add_edge(1, 2, "INCLUDES", 1.0, "{}")?;
    conn.graph_add_edge(2, 3, "EXPLAINS", 0.9, "{}")?;
    conn.graph_add_edge(1, 4, "ESPIONAGE_TARGET", 0.8, "{}")?; // Cross-tenant boundary edge!
    conn.graph_add_edge(4, 5, "PROPELLED_BY", 1.0, "{}")?;

    let query_vector = [0.93, 0.07, 0.00];

    // --- Scenario 1: Strict Multi-Tenant Isolation for "cyberdyne" ---
    let config_cyberdyne = GraphRagConfig::default()
        .with_seeds(3)
        .with_max_hops(2)
        .with_limit(10)
        .with_tenant("cyberdyne")
        .with_roles(["admin", "analyst"]);

    let res_cyberdyne = conn.graph_rag_query("Genesis project", Some(&query_vector), &config_cyberdyne)?;
    let retrieved_ids: Vec<u64> = res_cyberdyne.results.iter().map(|r| r.entity_id).collect();

    // Node 4 (Weyland) has highest cosine similarity (0.95 vs 0.93), but MUST be strictly blocked!
    assert!(!retrieved_ids.contains(&4), "Tenant Weyland Node 4 must not leak into Cyberdyne context");
    assert!(!retrieved_ids.contains(&5), "Tenant Weyland Node 5 must not leak into Cyberdyne context");
    assert!(retrieved_ids.contains(&1), "Cyberdyne Node 1 should be retrieved");
    assert!(retrieved_ids.contains(&2), "Cyberdyne Node 2 should be retrieved");

    // Traversal must NOT follow edge (1) -> (4) into unauthorized tenant
    for r in &res_cyberdyne.results {
        for e in &r.related_edges {
            assert_ne!(e.to_id, 4, "Traversal must not leak cross-tenant edge to node 4");
            assert_ne!(e.from_id, 4, "Traversal must not leak cross-tenant edge from node 4");
        }
    }
    assert!(res_cyberdyne.prompt_context.contains("Tenant: `cyberdyne`"));

    // --- Scenario 2: Strict Multi-Tenant Isolation for "weyland" ---
    let config_weyland = GraphRagConfig::default()
        .with_seeds(3)
        .with_max_hops(2)
        .with_limit(10)
        .with_tenant("weyland")
        .with_roles(["admin", "analyst"]);

    let res_weyland = conn.graph_rag_query("Prometheus mission", Some(&query_vector), &config_weyland)?;
    let weyland_ids: Vec<u64> = res_weyland.results.iter().map(|r| r.entity_id).collect();
    assert!(weyland_ids.contains(&4), "Weyland Node 4 should be retrieved");
    assert!(weyland_ids.contains(&5), "Weyland Node 5 should be retrieved");
    assert!(!weyland_ids.contains(&1), "Cyberdyne Node 1 must not appear in Weyland query");
    assert!(!weyland_ids.contains(&2), "Cyberdyne Node 2 must not appear in Weyland query");

    // --- Scenario 3: Role-Based Access Control (Analyst vs Admin) ---
    // User only has "analyst" role. Node 1 requires "admin", so it must be filtered out.
    let config_analyst = GraphRagConfig::default()
        .with_seeds(3)
        .with_max_hops(2)
        .with_limit(10)
        .with_tenant("cyberdyne")
        .with_roles(["analyst"]);

    let res_analyst = conn.graph_rag_query("Genesis AI", Some(&query_vector), &config_analyst)?;
    let analyst_ids: Vec<u64> = res_analyst.results.iter().map(|r| r.entity_id).collect();
    assert!(!analyst_ids.contains(&1), "Admin-only Node 1 must be invisible to analyst role");
    assert!(analyst_ids.contains(&2), "Node 2 permits analyst role");
    assert!(analyst_ids.contains(&3), "Node 3 has public visibility");

    // --- Scenario 4: Metadata Key-Value Filter ---
    let mut meta_filter = std::collections::HashMap::new();
    meta_filter.insert("mission".to_string(), serde_json::json!("deep_space"));

    let config_meta = GraphRagConfig::default()
        .with_tenant("weyland")
        .with_roles(["admin", "analyst"])
        .with_metadata_filter(meta_filter);

    let res_meta = conn.graph_rag_query("space mission", Some(&query_vector), &config_meta)?;
    for r in &res_meta.results {
        assert!(r.properties.contains("deep_space"), "All results must match metadata filter");
    }

    Ok(())
}
