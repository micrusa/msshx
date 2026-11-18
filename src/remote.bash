_msshx_clipboard() {
    local encoded
    encoded=$(command base64 | command tr -d '\r\n') || return
    printf '\033]52;c;%s\a' "$encoded" >/dev/tty
}

_msshx_request() {
    local operation=$1 value=$2 encoded token
    shift 2
    encoded=$(printf '%s' "$value" | command base64 | command tr -d '\r\n') || return
    token=$(<"${_msshx_socket%/*}/token") || return
    [[ $token =~ ^[0-9a-f]{64}$ ]] || return 1
    printf 'header = "Authorization: Bearer %s"\n' "$token" |
        command curl --disable --config - --silent --show-error --fail-with-body --globoff \
        --noproxy '*' --unix-socket "$_msshx_socket" \
        --header "X-Msshx-Value: $encoded" "$@" "http://localhost/$operation"
}

_msshx_client_path() {
    if [[ $1 == "$HOME/"* ]]; then
        printf '~/%s' "${1#"$HOME/"}"
    else
        printf '%s' "$1"
    fi
}

_msshx_open() {
    if [[ $# != 1 ]]; then
        printf 'Usage: copen URL\n' >&2
        return 2
    fi
    _msshx_request open "$1" --request POST
}

_msshx_push() {
    if [[ $# -lt 1 || $# -gt 2 || -z ${1##*/} ]]; then
        printf 'Usage: cpush CLIENT_FILE [REMOTE_FILE]\n' >&2
        return 2
    fi
    local destination=${2:-./${1##*/}} temp
    [[ $destination == /* ]] || destination=$PWD/$destination
    temp=$(command mktemp "${destination%/*}/.msshx.XXXXXXXX") || return
    if _msshx_request from-client "$(_msshx_client_path "$1")" --output "$temp"; then
        if [[ ! -d $destination ]] && command mv -f -- "$temp" "$destination"; then
            return
        fi
    fi
    command rm -f -- "$temp"
    return 1
}

_msshx_pull() {
    if [[ $# -lt 1 || $# -gt 2 || ! -f $1 ]]; then
        printf 'Usage: cpull REMOTE_FILE [CLIENT_FILE]\n' >&2
        return 2
    fi
    local source=$1 destination
    [[ $source == /* ]] || source=$PWD/$source
    if [[ $# == 2 ]]; then
        destination=$(_msshx_client_path "$2")
    else
        destination="~/Downloads/${1##*/}"
    fi
    _msshx_request to-client "$destination" --upload-file "$source"
}

_msshx_forward() {
    if [[ $# != 1 ]]; then
        printf 'Usage: cfw PORT or cfw LOCAL_PORT:REMOTE_PORT\n' >&2
        return 2
    fi
    _msshx_request forward "$1" --request POST
}

builtin alias clipboard='_msshx_clipboard' cb='_msshx_clipboard'
builtin alias cfw='_msshx_forward' cforward='_msshx_forward'
builtin alias copen='_msshx_open' clientopen='_msshx_open'
builtin alias cpush='_msshx_push' clientpush='_msshx_push'
builtin alias cpull='_msshx_pull' clientpull='_msshx_pull'
shopt -s expand_aliases
