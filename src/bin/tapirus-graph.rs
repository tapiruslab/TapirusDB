//! Standalone graph entry point for the first-party Graph edition.

use std::env;
use tapirus::Direction;
use tapirus::editions::GraphDatabase;

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() == 2 && args[1] == "--version" {
        println!("tapirus-graph {}", tapirus::VERSION);
        return Ok(());
    }
    if args.len() < 4 {
        return Err("Usage: tapirus-graph <database.tapir> node <ID> <LABEL> <JSON> | edge <FROM> <TO> <LABEL> | neighbors <ID>".into());
    }

    let db = GraphDatabase::open(&args[1])?;
    match (args[2].as_str(), args.len()) {
        ("node", 6) => {
            let properties: serde_json::Value = serde_json::from_str(&args[5])?;
            db.add_node(args[3].parse()?, &args[4], &properties.to_string())?;
            println!("ok");
        }
        ("edge", 6) => {
            println!(
                "{}",
                db.add_edge(args[3].parse()?, args[4].parse()?, &args[5], 1.0, "{}")?
            );
        }
        ("neighbors", 4) => {
            println!(
                "{}",
                serde_json::to_string(&db.neighbors(args[3].parse()?, Direction::Both, None))?
            );
        }
        _ => return Err("Invalid graph operation or argument count".into()),
    }
    db.close()?;
    Ok(())
}
