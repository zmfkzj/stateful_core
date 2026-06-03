fn main() {
    if let Err(error) = stateful_cli::run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
