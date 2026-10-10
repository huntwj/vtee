#!/bin/sh
# Run arithmetic.ndjson and fail unless every wait matched and the program exited with code 7.
# A script exits 0 even when a wait_for times out, so the test has to read the results itself.
set -eu
cd "$(dirname "$0")"

out=$(vtee --script arithmetic.ndjson --env 'PS1=$ ' -- bash --norc)

case $out in
    *'"matched":false'*) echo "FAIL: a wait_for timed out"; exit 1 ;;
esac
case $out in
    *'"cmd":"wait_exit","running":false,"code":7,"timed_out":false'*) echo "PASS" ;;
    *) echo "FAIL: the program did not exit with code 7"; exit 1 ;;
esac
