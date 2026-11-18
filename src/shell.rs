use std::env;
use std::path::Path;
use std::process::Command;

pub fn local_config() -> String {
    let mut config = String::new();
    if let Some(home) = env::var_os("HOME") {
        let bashrc = Path::new(&home).join(".bashrc");
        if let Ok(output) = Command::new("bash")
            .args([
                "-c",
                r#"test -f "$1" && exec bash --noprofile --rcfile "$1" -ic 'builtin alias -p >&3' 3>&1 >/dev/null 2>&1"#,
                "msshx",
            ])
            .arg(bashrc)
            .output()
            && output.status.success()
        {
            let aliases = String::from_utf8_lossy(&output.stdout);
            // Bash quotes each alias value, including any embedded newlines.
            for definition in alias_definitions(&aliases) {
                if !definition.starts_with("alias msshx=")
                    && definition.trim_end() != "alias ssh='msshx'"
                {
                    config.push_str("builtin ");
                    config.push_str(definition);
                }
            }
        }
    }

    for role in ["AUTHOR", "COMMITTER"] {
        let var = format!("GIT_{role}_IDENT");
        let Ok(output) = Command::new("git").args(["var", &var]).output() else {
            continue;
        };
        if !output.status.success() {
            continue;
        }

        let ident = String::from_utf8_lossy(&output.stdout);
        if let Some((identity, date)) = ident.trim_end().rsplit_once("> ")
            && let Some((name, email)) = identity.rsplit_once(" <")
        {
            for (field, value) in [("NAME", name), ("EMAIL", email), ("DATE", date)] {
                config.push_str(&format!("export GIT_{role}_{field}={}\n", quote(value)));
            }
        }
    }
    config
}

fn alias_definitions(aliases: &str) -> impl Iterator<Item = &str> {
    let mut start = 0;
    let mut quoted = false;
    let mut escaped = false;
    aliases
        .char_indices()
        .filter_map(move |(index, character)| {
            if escaped {
                escaped = false;
            } else if character == '\'' {
                quoted = !quoted;
            } else if !quoted {
                if character == '\\' {
                    escaped = true;
                } else if character == '\n' {
                    let definition = &aliases[start..=index];
                    start = index + 1;
                    return Some(definition);
                }
            }
            None
        })
}

pub fn remote_command(remote: &str, config: &str) -> String {
    let rc = format!(
        "if [[ -r ~/.bashrc ]]; then source ~/.bashrc; fi\nbuiltin shopt -u expand_aliases\n_msshx_socket={}\n{config}{}",
        quote(&format!("{remote}/client.sock")),
        include_str!("remote.bash"),
    );
    let bootstrap = format!("exec bash --rcfile <(printf %s {}) -i", quote(&rc));
    format!("exec bash -c {}", quote(&bootstrap))
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
