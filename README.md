# Auto-Tunnel

Auto-Tunnel is a simple tool that allows you to tunnel all registered ports (>=1024) from remote
machine to your local machine without exposing all of them to the internet. It's useful for
developers who works in a remote machine that need access to the services that they run for
development and testing purposes.

## Features

- [x] Dynamically tunnel ports from remote to local machine without any configuration
- [x] Support multiple remote machine
- [ ] Support wss protocol
- [ ] Set a static list of port to tunnel

## Building from source

Auto-Tunnel is not distributed in any package manager yet, so you need to build it from source. You
will need to have Rust installed in your machine to build this project. Please refer to
[Rust installation guide](https://www.rust-lang.org/tools/install) to install Rust in your machine.

For windows build, you will need to install `mingw-w64-gcc` and `x86_64-pc-windows-gnu` target for
Rust.

To build the project, you can run the following command:

```bash
git clone https://github.com/aguss787/auto-tunnel.git
cd auto-tunnel

# Build linux client and server
make linux

# Build windows client only
make windows-client

# Build all
make all
```

Once the build process is finished, you can find the binary in `target/release` directory.

```bash
mv target/release/server /usr/local/bin/auto-tunnel-server
mv target/release/client /usr/local/bin/auto-tunnel-client
mv target/x86_64-pc-windows-gnu/release/client.exe /usr/local/bin/auto-tunnel-client.exe
```

## Usage

### Server

#### Running in CLI

To run the server in CLI, you can run the following command:

```bash
auto-tunnel-server
```

You can use `--help` to see more options.

#### Running as a systemd services

You can use the following service file template to run the server as a systemd service.

```
[Unit]
Description=Auto-Tunnel server
After=network-online.target
Wants=network-online.target

[Service]
TimeoutStartSec=0
Type=simple
ExecStart=/usr/local/bin/auto-tunnel-server
Restart=on-failure
RestartSec=5s

[Install]
WantedBy=multi-user.target
```

### Client

#### Running in CLI

To run the client in CLI, you can run the following command:

```bash
auto-tunnel-client --remote ws://<server-ip>:<server-port>
```
