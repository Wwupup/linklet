//! A real executable for exact argument and stdin tests.
use std::io::Read;
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("exit") {
        std::process::exit(args[1].parse().expect("exit code"));
    }
    if args.first().map(String::as_str) == Some("sleep") {
        std::thread::sleep(std::time::Duration::from_millis(args[1].parse().expect("milliseconds")));
        return;
    }
    println!("cwd:{}", std::env::current_dir().expect("cwd").display());
    for arg in args { println!("arg:{}:{arg}", arg.len()); }
    let mut input = Vec::new();
    std::io::stdin().read_to_end(&mut input).expect("closed stdin");
    println!("stdin:{}", input.len());
}
