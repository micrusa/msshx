# mSSHx

A vibe-coded lightweight Rust wrapper for interactive SSH.

### Installation

Run `./build.sh` to build or `./install.sh` to install in `~/.local/bin` (override with `PREFIX`). Put that directory on `PATH`, then run `msshx user@host`. Run `./alias.sh` and `source ~/.bashrc` to use `ssh` as an alias.

## Features

### Aliases & Git

Interactive sessions load local `~/.bashrc` aliases, excluding `msshx` and the wrapper’s `ssh` alias. Git author and committer names, emails, and dates come from local `git var` when available. These Git dates stay fixed for the session.

### Remote injected commands

| Remote command | Action |
| --- | --- |
| `cat hello \| cb` | Copy to the client clipboard with OSC 52. Also `clipboard`. |
| `cfw 8080` or `cfw 8080:8081` | Forward client loopback to remote loopback using `LOCAL_PORT:REMOTE_PORT` until SSH exits. Also `cforward`. |
| `copen https://example.com` | Open a URL on the client. Also `clientopen`. |
| `copen 127.0.0.1:8080` | Forward the remote loopback URL to an available client port and open it. The forward lasts until SSH exits. |
| `cpush ~/Documents/file.pdf [remote-file]` | Copy a client file to the remote current directory or the given remote filename. Also `clientpush`. |
| `cpull ./path [client-path]` or `cpull * [client-dir/]` | Copy remote file(s) or folder(s) to the client’s `~/Downloads`, a client path, or a client directory. Also `clientpull`. |

Destination directories must exist. Relative client paths start where you launched `msshx`. Clipboard copying needs terminal support for OSC 52 and uses no tunnel. Paste with your terminal’s usual shortcut.

### Security

Of course giving access to every file in client through a socket is not safe, so:

Each socket request carries a session token and prompts in your SSH terminal. Press Enter or type `y` to allow, or type `n` to decline. The token is available to the remote account, so approve only requests you initiated. OSC 52 clipboard copy is controlled by your terminal.

## Requirements

Requires OpenSSH on the client, Bash and curl 7.76+ on the server, and `xdg-open` or macOS `open` for URLs. File transfers, URL opening, and port forwarding requests use a private reverse Unix socket. Use plain SSH for hosts where you do not want these integrations.
