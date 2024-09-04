use auto_tunnel::server::Server;
use clap::Parser;

#[derive(Parser)]
struct ServerArgs {
    #[clap(short, long)]
    whitelist: Vec<u16>,
    #[clap(short, long)]
    blacklist: Vec<u16>,

    bind: Option<String>,
}

fn main() -> std::io::Result<()> {
    if std::env::var("AT_LOG").is_err() {
        std::env::set_var("AT_LOG", "INFO");
    }

    let args = ServerArgs::parse();
    pretty_env_logger::formatted_builder()
        .parse_env("AT_LOG")
        .init();

    if !args.whitelist.is_empty() && !args.blacklist.is_empty() {
        panic!("Cannot have both a whitelist and a blacklist");
    }

    let server = if !args.blacklist.is_empty() {
        Server::with_blacklist(args.blacklist)
    } else if !args.whitelist.is_empty() {
        Server::with_whitelist(args.whitelist)
    } else {
        Server::new()
    };

    let bind = args.bind.as_deref().unwrap_or("0.0.0.0:3000");

    tracing::info!("listening on ws://{bind}");
    server.run(bind)
}
