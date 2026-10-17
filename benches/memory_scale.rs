//! Actual persisted memory API, with independent queries and exact ground truth.
#![forbid(unsafe_code)]
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{collections::HashSet, time::Instant};
use tapirus::{Connection, MemoryConfig, MemoryDocument, MemorySearch};

fn vector(seed: &mut u64, dims: usize) -> Vec<f32> {
    let mut values: Vec<f32> = (0..dims)
        .map(|_| {
            *seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((*seed >> 32) as u32 as f64 / u32::MAX as f64 * 2.0 - 1.0) as f32
        })
        .collect();
    let norm = values
        .iter()
        .map(|v| f64::from(*v).powi(2))
        .sum::<f64>()
        .sqrt();
    for v in &mut values {
        *v = (f64::from(*v) / norm) as f32;
    }
    values
}
fn setting(name: &str, default: usize, min: usize, max: usize) -> usize {
    let value = std::env::var(name).map_or(default, |v| v.parse().expect("integer setting"));
    assert!((min..=max).contains(&value), "{name} out of range");
    value
}
fn percentiles(mut values: Vec<f64>) -> serde_json::Value {
    values.sort_by(f64::total_cmp);
    let p = |q: f64| values[((values.len() - 1) as f64 * q).ceil() as usize];
    json!({"p50":p(0.5),"p95":p(0.95),"p99":p(0.99)})
}
fn peak_rss() -> Option<u64> {
    #[cfg(target_os = "windows")]
    {
        let output = std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-Command",
                &format!("(Get-Process -Id {}).PeakWorkingSet64", std::process::id()),
            ])
            .output()
            .ok()?;
        String::from_utf8_lossy(&output.stdout).trim().parse().ok()
    }
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/self/status")
            .ok()?
            .lines()
            .find_map(|line| {
                line.strip_prefix("VmHWM:")
                    .and_then(|v| v.split_whitespace().next()?.parse::<u64>().ok())
                    .map(|v| v * 1024)
            })
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        None
    }
}
fn main() {
    if cfg!(debug_assertions) {
        println!("Run cargo bench --bench memory_scale for measurements.");
        return;
    }
    let count = setting("TAPIRUS_BENCH_DOCS", 100_000, 100, 1_000_000);
    let dims = setting("TAPIRUS_BENCH_DIMS", 64, 2, 4096);
    let queries = setting("TAPIRUS_BENCH_QUERIES", 100, 1, 1000);
    let ef = setting("TAPIRUS_BENCH_EF", 128, 16, 4096);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("scale.tapir");
    let db = Connection::open(&path).unwrap();
    let config = MemoryConfig {
        ef_search: ef,
        ..MemoryConfig::research()
    };
    db.configure_memory(config.clone()).unwrap();
    let mut seed = 0x1234_abcd_5678_9012u64;
    let mut digest = Sha256::new();
    let start = Instant::now();
    for offset in (0..count).step_by(1000) {
        let documents: Vec<_> = (offset..(offset + 1000).min(count))
            .map(|id| {
                let v = vector(&mut seed, dims);
                for component in &v {
                    digest.update(component.to_le_bytes());
                }
                MemoryDocument {
                    content: format!(
                        "Synthetic source {id} device group {} maintenance calibration",
                        id % 100
                    ),
                    external_id: Some(format!("source-{id}")),
                    embedding: Some(v),
                    embedding_model: Some("benchmark/uniform-normalized-v1".into()),
                    namespace: Some("scale".into()),
                    ..Default::default()
                }
            })
            .collect();
        db.store_memory_batch(&documents).unwrap();
        if offset % 10_000 == 0 {
            eprintln!(
                "persisted {} / {count}",
                (offset + documents.len()).min(count)
            );
        }
    }
    db.close().unwrap();
    drop(db);
    let ingestion = start.elapsed().as_secs_f64();
    let start = Instant::now();
    let db = Connection::open_with_config(
        &path,
        tapirus::Config {
            memory: config,
            ..Default::default()
        },
    )
    .unwrap();
    let reopen = start.elapsed().as_secs_f64();
    let start = Instant::now();
    let stats = db.prepare_memory_indexes().unwrap();
    let index_seconds = start.elapsed().as_secs_f64();
    let mut ann_latency = Vec::new();
    let mut exact_latency = Vec::new();
    let mut recall = Vec::new();
    let mut candidate_max = 0;
    let mut sweep: std::collections::BTreeMap<usize, (Vec<f64>, Vec<f64>)> = [512usize, 1024]
        .into_iter()
        .filter(|budget| *budget > ef)
        .map(|budget| (budget, (Vec::new(), Vec::new())))
        .collect();
    for i in 0..queries {
        let q = vector(&mut seed, dims);
        for component in &q {
            digest.update(component.to_le_bytes());
        }
        let mut request = MemorySearch {
            embedding: Some(q),
            embedding_model: Some("benchmark/uniform-normalized-v1".into()),
            namespace: Some("scale".into()),
            limit: 10,
            candidate_limit: ef,
            vector_weight: 1.0,
            lexical_weight: 0.0,
            recency_weight: 0.0,
            importance_weight: 0.0,
            ..Default::default()
        };
        let start = Instant::now();
        let ann = db.search_memories_detailed(&request).unwrap();
        ann_latency.push(start.elapsed().as_secs_f64() * 1000.0);
        candidate_max = candidate_max.max(ann.diagnostics.scored_candidates);
        request.exact = true;
        let start = Instant::now();
        let exact = db.search_memories(&request).unwrap();
        exact_latency.push(start.elapsed().as_secs_f64() * 1000.0);
        let expected: HashSet<_> = exact.iter().map(|r| r.entry.id).collect();
        recall.push(
            ann.results
                .iter()
                .filter(|r| expected.contains(&r.entry.id))
                .count() as f64
                / 10.0,
        );
        for (&budget, (latencies, recalls)) in &mut sweep {
            request.exact = false;
            request.candidate_limit = budget;
            let started = Instant::now();
            let response = db.search_memories_detailed(&request).unwrap();
            latencies.push(started.elapsed().as_secs_f64() * 1000.0);
            assert!(response.diagnostics.scored_candidates <= budget);
            recalls.push(
                response
                    .results
                    .iter()
                    .filter(|r| expected.contains(&r.entry.id))
                    .count() as f64
                    / 10.0,
            );
        }
        if i % 25 == 0 {
            eprintln!("evaluated {} / {queries}", i + 1);
        }
    }
    assert!(candidate_max <= ef);
    assert_eq!(stats.documents, count);
    assert_eq!(stats.indexed_vectors, count);
    let sweep:Vec<_> = sweep.into_iter().map(|(budget,(latencies,recalls))|json!({"candidate_limit":budget,"ann_latency_ms":percentiles(latencies),"recall_at_10":recalls.iter().sum::<f64>()/queries as f64})).collect();
    let mut source = Sha256::new();
    for bytes in [
        include_bytes!("../src/vector/hnsw.rs").as_slice(),
        include_bytes!("../src/memory/indexed.rs").as_slice(),
        include_bytes!("../src/memory/config.rs").as_slice(),
        include_bytes!("../src/memory/api.rs").as_slice(),
    ] {
        source.update(bytes);
    }
    let report = json!({"version":tapirus::VERSION,"scope":"Synthetic normalized uniform vectors, independent queries; memory API with persistence/reopen. Not neural relevance evidence.","documents":count,"dimensions":dims,"queries":queries,"k":10,"ef_search":ef,"candidate_limit":ef,"candidate_sweep":sweep,"retrieval_source_sha256":format!("{:x}",source.finalize()),"source_query_sha256":format!("{:x}",digest.finalize()),"generator_sha256":format!("{:x}",Sha256::digest(include_bytes!("memory_scale.rs"))),"ingestion_seconds":ingestion,"reopen_seconds":reopen,"index_build_seconds":index_seconds,"ann_latency_ms":percentiles(ann_latency),"exact_latency_ms":percentiles(exact_latency),"recall_at_10":recall.iter().sum::<f64>()/queries as f64,"minimum_query_recall":recall.iter().copied().fold(1.0,f64::min),"scored_candidates_max":candidate_max,"peak_process_rss_bytes":peak_rss(),"database_bytes":std::fs::metadata(&path).unwrap().len(),"memory_stats":stats,"host":{"os":std::env::consts::OS,"arch":std::env::consts::ARCH,"cpu":std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_default()}});
    std::fs::create_dir_all("target").unwrap();
    let output = std::env::var("TAPIRUS_BENCH_OUTPUT")
        .unwrap_or_else(|_| "target/memory-scale-results.json".into());
    std::fs::write(
        &output,
        serde_json::to_string_pretty(&report).unwrap() + "\n",
    )
    .unwrap();
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
}
