#!/bin/sh
set -eu
bashrc=$HOME/.bashrc
line="alias ssh='msshx'"
if [ ! -f "$bashrc" ] || ! grep -Fqx "$line" "$bashrc"; then
    printf '\n%s\n' "$line" >> "$bashrc"
fi
printf 'Run source ~/.bashrc to enable the alias.\n'
