//! Console entry point for the target agent.
fn main() {
    use linklet_agent::{Agent, Invocation, Options, USAGE};
    let result = match Options::parse(std::env::args().skip(1)) {
        Ok(Invocation::Help) => {
            print!("{USAGE}");
            return;
        }
        Ok(Invocation::Version) => {
            println!("linklet-agent {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        Ok(Invocation::Start(options)) => Agent::start(options).and_then(|agent| {
            let info = agent.info();
            for warning in &info.warnings {
                eprintln!("linklet-agent: {warning}");
            }
            println!("{}", info.banner());
            println!("Pairing code: {}", info.pairing_code);
            let (_keep_running, shutdown) = std::sync::mpsc::channel();
            agent
                .serve(shutdown)
                .map_err(|message| linklet_agent::StartupError {
                    exit_code: 1,
                    message,
                })
        }),
        Err(error) => Err(error),
    };
    if let Err(error) = result {
        eprintln!("linklet-agent: {}", error.message);
        std::process::exit(error.exit_code);
    }
}
