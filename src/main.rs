fn main() {
    if let Err(error) = thermal_cli::run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}
