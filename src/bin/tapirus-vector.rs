//! Standalone vector entry point for the first-party Vector edition.

use std::env;
use tapirus::editions::VectorDatabase;

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() == 2 && args[1] == "--version" {
        println!("tapirus-vector {}", tapirus::VERSION);
        return Ok(());
    }
    if args.len() < 6 {
        return Err("Usage: tapirus-vector <database.tapir> <collection> <dimension> insert <ID> <VECTOR_JSON> <METADATA_JSON> | search <VECTOR_JSON> <LIMIT>".into());
    }

    let db = VectorDatabase::open(&args[1])?;
    let collection = db.collection(&args[2], args[3].parse()?)?;
    match (args[4].as_str(), args.len()) {
        ("insert", 8) => {
            let embedding: Vec<f32> = serde_json::from_str(&args[6])?;
            let metadata: serde_json::Value = serde_json::from_str(&args[7])?;
            collection.insert(args[5].parse()?, &embedding, &metadata)?;
            println!("ok");
        }
        ("search", 7) => {
            let embedding: Vec<f32> = serde_json::from_str(&args[5])?;
            let rows = collection.search(&embedding, args[6].parse()?)?;
            let values: Vec<serde_json::Value> = rows
                .iter()
                .map(|row| {
                    let fields: serde_json::Map<String, serde_json::Value> = row
                        .columns()
                        .iter()
                        .zip(row.values())
                        .map(|(name, value)| {
                            (
                                name.clone(),
                                serde_json::to_value(value).unwrap_or_default(),
                            )
                        })
                        .collect();
                    serde_json::Value::Object(fields)
                })
                .collect();
            println!("{}", serde_json::to_string(&values)?);
        }
        _ => return Err("Invalid vector operation or argument count".into()),
    }
    drop(collection);
    db.close()?;
    Ok(())
}
