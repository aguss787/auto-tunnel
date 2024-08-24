use auto_tunnel::client::Client;
use clap::Parser;

#[derive(Parser)]
struct ClientArgs {
    #[arg(short, long)]
    port_offset: Option<u16>,

    #[arg(short, long, default_value = "false")]
    dry_run: bool,

    server: String,
}

fn main() -> std::io::Result<()> {
    if std::env::var("AT_LOG").is_err() {
        std::env::set_var("AT_LOG", "INFO");
    }

    let args = ClientArgs::parse();
    pretty_env_logger::formatted_builder()
        .parse_env("AT_LOG")
        .init();

    let client = Client::new().set_port_offset(args.port_offset.unwrap_or_default());
    client.run(&args.server, args.dry_run)
}
