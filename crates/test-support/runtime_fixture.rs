//! Interpreter stand-in: no dependency on installed Python or PowerShell.
use std::io::Write;

fn main() {
    let program = std::env::current_exe().expect("executable");
    let mode = std::fs::read_to_string(program.with_extension("mode")).unwrap_or_default();
    match mode.trim() {
        "fail" => std::process::exit(7),
        "hang" => {
            std::thread::sleep(std::time::Duration::from_secs(30));
            return;
        }
        "invalid" => {
            println!("not an interpreter reply");
            return;
        }
        "flood" => {
            let _ = std::io::stdout().write_all(&vec![b'x'; 1024 * 1024]);
            return;
        }
        _ => {}
    }
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a.contains("linklet_runtime")) {
        let program = program.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");
        println!("{{\"linklet_runtime\":1,\"program\":\"{program}\",\"version\":\"3.13.1\"}}");
    } else {
        println!("script executed");
    }
}
