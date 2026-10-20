//! Standalone relational entry point for the first-party Lite edition.

use std::env;
use tapirus::editions::LiteDatabase;

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() == 2 && args[1] == "--version" {
        println!("tapirus-lite {}", tapirus::VERSION);
        return Ok(());
    }
    if args.len() != 4 || !matches!(args[2].as_str(), "--exec" | "--query") {
        return Err("Usage: tapirus-lite <database.tapir> --exec <SQL> | --query <SQL>".into());
    }

    let db = LiteDatabase::open(&args[1])?;
    if args[2] == "--exec" {
        println!("{}", db.execute(&args[3])?);
    } else {
        let rows = db.query(&args[3])?;
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
    db.close()?;
    Ok(())
}
