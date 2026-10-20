//! Standalone JSON collection entry point for the first-party Document edition.

use std::env;
use tapirus::editions::DocumentDatabase;

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() == 2 && args[1] == "--version" {
        println!("tapirus-document {}", tapirus::VERSION);
        return Ok(());
    }
    if args.len() < 4 {
        return Err("Usage: tapirus-document <database.tapir> <collection> <insert JSON|get ID|list|delete ID|count>".into());
    }

    let db = DocumentDatabase::open(&args[1])?;
    let collection = db.collection(&args[2])?;
    match (args[3].as_str(), args.get(4)) {
        ("insert", Some(json)) if args.len() == 5 => {
            let document: serde_json::Value = serde_json::from_str(json)?;
            println!("{}", collection.insert_one(&document)?);
        }
        ("get", Some(id)) if args.len() == 5 => {
            println!(
                "{}",
                serde_json::to_string(&collection.find_by_id(id.parse()?)?)?
            );
        }
        ("list", None) => {
            println!("{}", serde_json::to_string(&collection.find_all()?)?);
        }
        ("delete", Some(id)) if args.len() == 5 => {
            println!("{}", collection.delete(id.parse()?)?);
        }
        ("count", None) => {
            println!("{}", collection.count()?);
        }
        _ => return Err("Invalid document operation or argument count".into()),
    }
    drop(collection);
    db.close()?;
    Ok(())
}
