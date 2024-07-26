use clap::Parser;
use auto_tunnel::server::Server;

#[derive(Parser)]
struct ServerArgs {
    bind: Option<String>,
}

fn main() -> std::io::Result<()> {
    if std::env::var("WT_LOG").is_err() {
        std::env::set_var("WT_LOG", "INFO");
    }

    let args = ServerArgs::parse();
    pretty_env_logger::formatted_builder()
        .parse_env("WT_LOG")
        .init();

    let server = Server::new();
    let bind = args.bind.as_deref().unwrap_or("0.0.0.0:3000");

    tracing::info!("listening on ws://{bind}");
    server.run(bind)
}
