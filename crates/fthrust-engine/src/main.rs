//! `fthrust-engine` binary entry point.
#![forbid(unsafe_code)]

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--version") {
        println!("fthrust-engine {}", fthrust_engine::VERSION);
        return;
    }
    eprintln!("usage: fthrust-engine --socket PATH");
    std::process::exit(2);
}
