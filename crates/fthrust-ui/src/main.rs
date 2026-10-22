//! `fthrust` binary entry point.
#![forbid(unsafe_code)]

fn main() {
    fthrust_media::capabilities::handle_probe_argv();
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--version") {
        println!("fthrust {}", fthrust_ui::VERSION);
        return;
    }
    eprintln!("fthrust: the application window arrives with plan 4 task 9");
    std::process::exit(2);
}
