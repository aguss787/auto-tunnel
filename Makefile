default: linux

linux:
	echo "Building for Linux"
	cargo build --release

windows-client:
	echo "Building for Windows client"
	cargo build --release --target=x86_64-pc-windows-gnu --no-default-features -F windows --bin client

all: linux windows-client

clean:
	cargo clean

