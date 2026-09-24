use tapirus::embedded::{
    FlashBlockDevice, MicroDatabase, MicroPager, RamBlockDevice,
    DEFAULT_MICRO_BLOCK_SIZE, MICRO_DATABASE_MAGIC,
};
use tapirus::Result;

#[test]
fn test_ram_block_device_read_write() -> Result<()> {
    let block_size = 512;
    let block_count = 16;
    let mut dev = RamBlockDevice::new(block_count, block_size);

    assert_eq!(dev.block_count(), 16);
    assert_eq!(dev.block_size(), 512);

    // Initial read is zeroed
    let mut buf = vec![0u8; block_size];
    dev.read_block(1, &mut buf)?;
    assert!(buf.iter().all(|&b| b == 0));

    // Write payload to block 1
    let mut write_buf = vec![0xABu8; block_size];
    write_buf[0..8].copy_from_slice(b"TESTDATA");
    dev.write_block(1, &write_buf)?;

    // Read back and verify
    let mut read_buf = vec![0u8; block_size];
    dev.read_block(1, &mut read_buf)?;
    assert_eq!(&read_buf[0..8], b"TESTDATA");
    assert_eq!(read_buf[8], 0xAB);

    // Out of bounds error handling
    assert!(dev.read_block(99, &mut read_buf).is_err());

    Ok(())
}

#[test]
fn test_micro_pager_initialization_and_header() -> Result<()> {
    let dev = RamBlockDevice::new(32, DEFAULT_MICRO_BLOCK_SIZE);
    let mut pager = MicroPager::open(dev)?;

    assert_eq!(pager.block_size(), DEFAULT_MICRO_BLOCK_SIZE);
    assert_eq!(pager.header().magic, MICRO_DATABASE_MAGIC);
    assert_eq!(pager.header().version, 1);
    assert_eq!(pager.header().total_blocks, 32);

    // Write a block through the pager
    let mut block_data = vec![0x42u8; DEFAULT_MICRO_BLOCK_SIZE];
    block_data[0..4].copy_from_slice(&12345u32.to_le_bytes());
    pager.write_block(2, &block_data)?;

    let read_back = pager.read_block(2)?;
    assert_eq!(read_back[0..4], 12345u32.to_le_bytes());
    assert_eq!(read_back[10], 0x42);

    Ok(())
}

#[test]
fn test_micro_database_sensor_logs_vectors_and_graphs() -> Result<()> {
    // 1. Open MicroDatabase with 64KB simulated flash (128 blocks of 512 bytes)
    let mut db = MicroDatabase::open_ram(128, 512)?;

    // 2. Store sensor records (time-series telemetry)
    for i in 1..=25 {
        db.store_sensor_record(
            1, // Sensor ID: Temperature
            1726848000 + i,
            24.0 + (i as f32 * 0.1),
            "Celsius",
        )?;
    }
    assert_eq!(db.record_count(), 25);

    // 3. Ingest quantized micro-vectors (e.g. LiDAR or acoustic signatures)
    let vec_door_ajar = [0.90, 0.10, 0.05, 0.02, 0.85, 0.12, 0.01, 0.00];
    let vec_window_open = [0.88, 0.12, 0.08, 0.01, 0.82, 0.15, 0.02, 0.00];
    let vec_temp_spike = [0.10, 0.90, 0.80, 0.70, 0.05, 0.02, 0.95, 0.90];

    db.add_micro_vector(101, &vec_door_ajar)?;
    db.add_micro_vector(102, &vec_window_open)?;
    db.add_micro_vector(103, &vec_temp_spike)?;
    assert_eq!(db.vector_count(), 3);

    // Query nearest micro-vector
    let query_vec = [0.89, 0.11, 0.06, 0.02, 0.84, 0.13, 0.01, 0.00];
    let results = db.search_micro_vector(&query_vec, 2);
    assert_eq!(results.len(), 2);
    // Nearest vector should be door_ajar (101) or window_open (102)
    assert!(results[0].0 == 101 || results[0].0 == 102);

    // 4. Link entities in embedded knowledge graph
    db.link_entities(1 /* Temp Sensor */, 201 /* Smart Thermostat */, "CONTROLS")?;
    db.link_entities(201 /* Smart Thermostat */, 301 /* HVAC AC Unit */, "TRIGGERS")?;

    let thermostat_links = db.get_entity_links(201);
    assert_eq!(thermostat_links.len(), 2);

    let connected_ids: Vec<u32> = thermostat_links.iter().map(|(id, _)| *id).collect();
    assert!(connected_ids.contains(&1));
    assert!(connected_ids.contains(&301));

    Ok(())
}
