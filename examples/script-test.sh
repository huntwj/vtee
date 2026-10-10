#!/bin/sh
# Run arithmetic.ndjson and fail unless every wait matched and the program exited with code 7.
# A wait_for that times out makes the run exit 1 (stderr says which line), so `set -e` covers it;
# the program's own exit code is in the wait_exit reply, which the test has to read itself.
set -eu
cd "$(dirname "$0")"

out=$(vtee --script arithmetic.ndjson --env 'PS1=$ ' -- bash --norc) || { echo "FAIL: vtee exited $?"; exit 1; }

case $out in
    *'"cmd":"wait_exit","running":false,"code":7,"timed_out":false'*) echo "PASS" ;;
    *) echo "FAIL: the program did not exit with code 7"; exit 1 ;;
esac
