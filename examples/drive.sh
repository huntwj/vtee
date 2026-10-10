#!/bin/sh
# Start a session, type a command into it, read the screen, end the session.
set -eu

NAME=drive-demo
SOCK="${TMPDIR:-/tmp}/vtee-$(id -u)/$NAME.sock"

vtee --headless --name "$NAME" --writable --env 'PS1=$ ' -- bash --norc &

tries=0
until vtee ls 2>/dev/null | grep -q "^$NAME[[:space:]].*live"; do
    tries=$((tries + 1))
    [ "$tries" -le 50 ] || { echo "session did not start" >&2; exit 1; }
    sleep 0.2
done

# $((40+2)) is typed, 42 is output: waiting for 42 cannot match the echo of the command.
printf '%s\n' \
    '{"cmd":"send","text":"echo $((40+2))\r"}' \
    '{"cmd":"wait_for","text":"42","timeout_ms":5000}' |
    vtee --connect "$SOCK" --text |
    sed 's/[[:space:]]*$//' | cat -s

vtee kill "$NAME"
wait
