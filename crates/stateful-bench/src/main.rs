fn main() {
    if let Err(error) = stateful_bench::run_cli() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
