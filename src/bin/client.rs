use clap::Parser;
use auto_tunnel::client::Client;

#[derive(Parser)]
struct ClientArgs {
    #[arg(short, long)]
    port_offset: Option<u16>,

    server: String,
}

fn main() -> std::io::Result<()> {
    if std::env::var("WT_LOG").is_err() {
        std::env::set_var("WT_LOG", "INFO");
    }

    let args = ClientArgs::parse();
    pretty_env_logger::formatted_builder()
        .parse_env("WT_LOG")
        .init();

    let client = Client::new().set_port_offset(args.port_offset.unwrap_or_default());
    client.run(&args.server)
}
