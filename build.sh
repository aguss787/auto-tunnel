#!/bin/bash

echo "Building for Linux"
cargo build --release

echo "Building for Windows client"
cargo build --release --target=x86_64-pc-windows-gnu --no-default-features -F windows --bin client

