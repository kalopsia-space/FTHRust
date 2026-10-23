//! The `uidrive` command line.

use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};
use uidrive::bench::Bench;
use uidrive::dump;
use uidrive::keys::parse_chord;
use uidrive::wl::{self, Button, Session};

#[derive(Parser)]
#[command(
    name = "uidrive",
    about = "Drives the FTHRust UI in the headless bench"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Capture the bench output as a PNG.
    Shot { name: Option<String> },
    /// Print `x y w h text` of a marker.
    Find {
        query: String,
        #[arg(long, default_value_t = 0)]
        nth: usize,
        #[arg(long)]
        all: bool,
    },
    /// Click a marker or `x,y`.
    Click {
        target: String,
        #[arg(long, default_value = "left")]
        button: String,
        #[arg(long)]
        double: bool,
        #[arg(long, default_value_t = 0)]
        nth: usize,
    },
    /// Move the pointer to a marker or `x,y`.
    Move { target: String },
    /// Drag from one target to another.
    Drag {
        from: String,
        to: String,
        #[arg(long, default_value_t = 12)]
        steps: usize,
    },
    /// Press a chord such as `Ctrl+Shift+S` or `F9`.
    Key { chord: String },
    /// Type text.
    Type { text: String },
    /// Wait for a marker to appear (or disappear with --gone).
    Wait {
        query: String,
        #[arg(long, default_value_t = 10.0)]
        timeout: f64,
        #[arg(long)]
        gone: bool,
    },
    /// Wait for any marker text to contain a substring.
    WaitText {
        text: String,
        #[arg(long, default_value_t = 10.0)]
        timeout: f64,
    },
    /// Wait for the status strip to equal a text.
    WaitStatus {
        text: String,
        #[arg(long, default_value_t = 10.0)]
        timeout: f64,
    },
    /// Print the current page, status, error bar and dialog.
    State {
        #[arg(long)]
        json: bool,
    },
    /// Tail the app (or engine) log.
    Log {
        #[arg(long)]
        engine: bool,
        #[arg(short = 'n', default_value_t = 40)]
        lines: usize,
    },
    /// Deliver a hotkey action through the app's own client.
    Hotkey { action: String },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli.cmd) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("uidrive: {e}");
            ExitCode::FAILURE
        }
    }
}

fn current_frame(bench: &Bench) -> Option<u64> {
    dump::read(&bench.dump).ok().map(|d| d.frame)
}

/// A marker name (centre) or `x,y`.
fn resolve(bench: &Bench, target: &str, nth: usize) -> Result<(f64, f64), String> {
    if let Some((x, y)) = target.split_once(',')
        && let (Ok(x), Ok(y)) = (x.trim().parse::<f64>(), y.trim().parse::<f64>())
    {
        return Ok((x, y));
    }
    let d = dump::read(&bench.dump)?;
    let m = dump::find(&d, target, nth).ok_or_else(|| format!("no marker matches '{target}'"))?;
    Ok(m.center())
}

fn print_marker(m: &dump::Marker) {
    println!(
        "{} {} {} {} {}",
        m.x,
        m.y,
        m.w,
        m.h,
        m.text.as_deref().unwrap_or("")
    );
}

fn run(cmd: Cmd) -> Result<(), String> {
    let bench = Bench::from_env()?;
    match cmd {
        Cmd::Find { query, nth, all } => {
            let d = dump::read(&bench.dump)?;
            if all {
                let mut n = 0;
                while let Some(m) = dump::find(&d, &query, n) {
                    print!("{} ", m.name);
                    print_marker(m);
                    n += 1;
                }
                if n == 0 {
                    return Err(format!("no marker matches '{query}'"));
                }
                return Ok(());
            }
            let m = dump::find(&d, &query, nth)
                .ok_or_else(|| format!("no marker matches '{query}'"))?;
            print_marker(m);
            Ok(())
        }
        Cmd::Wait {
            query,
            timeout,
            gone,
        } => {
            let after = current_frame(&bench);
            let d = dump::wait(
                &bench.dump,
                |d| dump::find(d, &query, 0).is_some() != gone,
                Duration::from_secs_f64(timeout),
                after,
                &format!("{}{query}", if gone { "absence of " } else { "" }),
            )?;
            if let Some(m) = dump::find(&d, &query, 0) {
                print_marker(m);
            }
            Ok(())
        }
        Cmd::WaitText { text, timeout } => {
            let after = current_frame(&bench);
            let d = dump::wait(
                &bench.dump,
                |d| {
                    d.markers
                        .iter()
                        .any(|m| m.text.as_deref().is_some_and(|t| t.contains(&text)))
                },
                Duration::from_secs_f64(timeout),
                after,
                &format!("text '{text}'"),
            )?;
            if let Some(m) = d
                .markers
                .iter()
                .find(|m| m.text.as_deref().is_some_and(|t| t.contains(&text)))
            {
                print!("{} ", m.name);
                print_marker(m);
            }
            Ok(())
        }
        Cmd::WaitStatus { text, timeout } => {
            let after = current_frame(&bench);
            dump::wait(
                &bench.dump,
                |d| d.status == text,
                Duration::from_secs_f64(timeout),
                after,
                &format!("status '{text}'"),
            )?;
            println!("{text}");
            Ok(())
        }
        Cmd::State { json } => {
            if json {
                let s = std::fs::read_to_string(&bench.dump).map_err(|e| e.to_string())?;
                println!("{s}");
                return Ok(());
            }
            let d = dump::read(&bench.dump)?;
            println!(
                "frame {} page {} status {:?} markers {}",
                d.frame,
                d.page,
                d.status,
                d.markers.len()
            );
            if let Some(e) = &d.error_bar {
                println!("error_bar {} {:?} {:?}", e.level, e.title, e.detail);
            }
            if let Some(t) = &d.dialog {
                println!("dialog {t:?}");
            }
            Ok(())
        }
        Cmd::Log { engine, lines } => {
            let path = bench
                .logs
                .join(if engine { "engine.log" } else { "app.log" });
            let s = std::fs::read_to_string(&path)
                .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            let all: Vec<&str> = s.lines().collect();
            for l in all.iter().skip(all.len().saturating_sub(lines)) {
                println!("{l}");
            }
            Ok(())
        }
        Cmd::Hotkey { action } => {
            let app = bench.app.clone().ok_or("BENCH_APP is not set")?;
            let status = std::process::Command::new(&app)
                .arg("--hotkey")
                .arg(&action)
                .env_clear()
                .env("HOME", bench.dir.join("home"))
                .env("XDG_RUNTIME_DIR", &bench.runtime_dir)
                .env("PATH", std::env::var("PATH").unwrap_or_default())
                .status()
                .map_err(|e| format!("cannot run {}: {e}", app.display()))?;
            if status.success() {
                Ok(())
            } else {
                Err(format!("hotkey client exited with {status}"))
            }
        }
        Cmd::Shot { name } => {
            let p = wl::shot(&bench, name.as_deref().unwrap_or("shot"))?;
            println!("{}", p.display());
            Ok(())
        }
        Cmd::Move { target } => {
            let (x, y) = resolve(&bench, &target, 0)?;
            Session::connect(&bench)?.move_to(x, y)
        }
        Cmd::Click {
            target,
            button,
            double,
            nth,
        } => {
            let (x, y) = resolve(&bench, &target, nth)?;
            let mut s = Session::connect(&bench)?;
            s.move_to(x, y)?;
            s.click(Button::parse(&button)?, double)
        }
        Cmd::Drag { from, to, steps } => {
            let a = resolve(&bench, &from, 0)?;
            let b = resolve(&bench, &to, 0)?;
            Session::connect(&bench)?.drag(a, b, steps)
        }
        Cmd::Key { chord } => {
            let c = parse_chord(&chord)?;
            Session::connect(&bench)?.key(&c)
        }
        Cmd::Type { text } => Session::connect(&bench)?.type_text(&text),
    }
}
