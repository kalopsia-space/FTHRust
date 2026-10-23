//! `fthrust-uploader`: one JSON request on stdin, one JSON line on stdout,
//! exit status 0 only when the response is ok.

use std::io::{Read, Write};

use fthrust_uploader::{Env, Response, handle};

fn main() {
    let mut input = String::new();
    let response = match std::io::stdin().read_to_string(&mut input) {
        Err(_) => Response::fail("Uploader request is not valid JSON."),
        Ok(_) => match serde_json::from_str(&input) {
            Err(_) => Response::fail("Uploader request is not valid JSON."),
            Ok(request) => run(request),
        },
    };
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{}", response.to_line());
    let _ = out.flush();
    std::process::exit(if response.ok { 0 } else { 1 });
}

fn run(request: serde_json::Value) -> Response {
    let env = match Env::from_process() {
        Ok(e) => e,
        Err(e) => return Response::fail(e),
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(r) => r,
        Err(e) => return Response::fail(format!("The uploader could not start: {e}")),
    };
    // A panic inside a provider still answers with a line.
    runtime
        .block_on(async move { tokio::spawn(async move { handle(&env, &request).await }).await })
        .unwrap_or_else(|_| Response::fail("The uploader stopped unexpectedly."))
}
