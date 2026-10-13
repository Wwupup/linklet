//! A real executable for exact argument and stdin tests.
use std::io::{Read, Write};
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("flood") {
        let bytes: usize = args[1].parse().expect("byte count");
        let writer = move |mut output: Box<dyn Write + Send>, marker: &[u8]| {
            let block = vec![b'x'; 65536];
            let mut remaining = bytes;
            while remaining > 0 {
                let size = remaining.min(block.len());
                output.write_all(&block[..size]).expect("write");
                remaining -= size;
            }
            output.write_all(marker).expect("marker");
        };
        let out = std::thread::spawn(move || writer(Box::new(std::io::stdout()), b"OUT-END"));
        let err = std::thread::spawn(move || writer(Box::new(std::io::stderr()), b"ERR-END"));
        out.join().expect("stdout"); err.join().expect("stderr");
        return;
    }
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
