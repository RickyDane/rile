#[cfg(unix)]
fn main() -> anyhow::Result<()> {
    rile_server::service::main()
}

#[cfg(not(unix))]
fn main() {
    rile_server::unsupported("rile-server");
}
