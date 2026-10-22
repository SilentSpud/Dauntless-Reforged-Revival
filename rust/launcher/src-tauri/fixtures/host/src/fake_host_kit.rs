//! A stand-in for `reforged-host` (Reforged's hostserver.test.ts fake `dist/cli.js`): `start` keeps the
//! control contract, `invite` answers one JSON line, anything else fails with an `Error:` line. A
//! first argument `fail` makes `start` fail; `deaf` makes it ignore the stop line.

use std::io::{BufRead, Write};

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mode = match args.first().map(String::as_str) {
        Some("fail" | "deaf") => Some(args.remove(0)),
        _ => None,
    };
    match args.first().map(String::as_str) {
        Some("start") => start(mode.as_deref()),
        Some("invite") => {
            println!("noise");
            let whom = args.iter().position(|a| a == "--for").and_then(|i| args.get(i + 1));
            let whom = whom.map_or("null".to_owned(), |w| format!("\"{w}\""));
            println!("{{\"link\":\"reforged-revival://join/abc\",\"for\":{whom}}}");
        }
        _ => {
            println!("working");
            eprintln!("\nError: game data is missing: reforged-host gamedata");
            std::process::exit(1);
        }
    }
}

fn start(mode: Option<&str>) {
    if std::env::var("REFORGED_CONTROL").as_deref() != Ok("stdin") {
        eprintln!("Error: not controlled");
        std::process::exit(2);
    }
    if mode == Some("fail") {
        eprintln!("\nError: port 27017 is already taken");
        std::process::exit(1);
    }
    println!("\u{1b}[32mmongod   | ready\u{1b}[0m");
    println!("REFORGED_SERVER_READY");
    std::io::stdout().flush().unwrap();
    if mode == Some("deaf") {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(60));
        }
    }
    for line in std::io::stdin().lock().lines() {
        match line {
            Ok(line) if line.trim() == "stop" => break,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    println!("Stopped.");
}
