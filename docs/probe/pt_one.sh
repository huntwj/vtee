#!/bin/sh
# usage: pt_one.sh TESTNAME [format]   (runs inside the node container with pilotty installed)
export TERM=xterm-256color
N="$1"; F="${2:-full}"
pilotty spawn --name t sh -c "sleep 1.5; cat /p/$N.bin; sleep 30" >/dev/null 2>&1

pilotty resize 40 12 -s t >/dev/null 2>&1 || pilotty resize 40 12 >/dev/null 2>&1
sleep 3.5
pilotty snapshot -s t -f "$F" 2>&1
pilotty kill -s t >/dev/null 2>&1
pilotty stop >/dev/null 2>&1
