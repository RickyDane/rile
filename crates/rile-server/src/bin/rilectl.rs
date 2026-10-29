#[cfg(unix)]
fn main() {
    rile_server::cli::main();
}

#[cfg(not(unix))]
fn main() {
    rile_server::unsupported("rilectl");
}
