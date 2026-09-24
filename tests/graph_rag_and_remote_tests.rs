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
