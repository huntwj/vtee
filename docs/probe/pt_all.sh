#!/bin/sh
export TERM=xterm-256color
for N in T1 T2 T3 T4 T5 T6 T7a T7b T8 T9 T10 T11 T12 T13 T14; do
  pilotty spawn --name t sh -c "sleep 1.5; cat /p/$N.bin; sleep 30" >/dev/null 2>&1
  pilotty resize 40 12 -s t >/dev/null 2>&1
  sleep 3.5
  pilotty snapshot -s t -f full > /p/pt_$N.json 2>/dev/null
  pilotty output -s t > /p/pt_out_$N.txt 2>/dev/null
  pilotty kill -s t >/dev/null 2>&1
  pilotty stop >/dev/null 2>&1
  sleep 2
done
echo done
