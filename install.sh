#!/bin/sh
set -eu
cd "$(dirname "$0")"
./build.sh
bin_dir=${PREFIX:-"$HOME/.local"}/bin
mkdir -p "$bin_dir"
install -m 755 target/release/msshx "$bin_dir/msshx"
printf 'Installed %s/msshx\n' "$bin_dir"
case ":$PATH:" in
    *":$bin_dir:"*) ;;
    *) printf 'Add %s to PATH.\n' "$bin_dir" ;;
esac
