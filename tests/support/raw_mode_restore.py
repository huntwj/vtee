"""Runs `vtee` on a real pty and checks the terminal is cooked again after it is signalled.

usage: raw_mode_restore.py <vtee binary> <TERM|KILL>

Exits 0 and prints OK on success; prints what went wrong to stderr and exits 1 otherwise. The
pty's slave end is held open here, so its termios stays inspectable after vtee and its
watchdog have exited.
"""

import fcntl
import os
import signal
import sys
import termios
import time


def lflag(fd):
    return termios.tcgetattr(fd)[3]


def wait_for(cond, what, timeout=10.0):
    deadline = time.time() + timeout
    while time.time() < deadline:
        if cond():
            return
        time.sleep(0.02)
    print(f"timed out waiting for {what}", file=sys.stderr)
    sys.exit(1)


def main():
    binary, sig_name = sys.argv[1], sys.argv[2]
    sig = {"TERM": signal.SIGTERM, "KILL": signal.SIGKILL}[sig_name]

    master, slave = os.openpty()
    cooked = termios.ICANON | termios.ECHO
    if lflag(master) & cooked != cooked:
        print("a fresh pty should start cooked", file=sys.stderr)
        sys.exit(1)

    pid = os.fork()
    if pid == 0:
        os.setsid()
        fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
        for target in (0, 1, 2):
            os.dup2(slave, target)
        os.close(master)
        os.close(slave)
        os.execv(binary, [binary, "--no-listen", "--", "cat"])

    # Keep draining output so vtee never blocks on a full pty buffer.
    os.set_blocking(master, False)

    def drain():
        try:
            os.read(master, 65536)
        except (BlockingIOError, OSError):
            pass

    def raw():
        drain()
        return lflag(master) & termios.ICANON == 0

    wait_for(raw, "vtee to put the terminal in raw mode")
    if lflag(master) & termios.ECHO:
        print("raw mode should have turned echo off", file=sys.stderr)
        sys.exit(1)

    os.kill(pid, sig)
    deadline = time.time() + 10
    while time.time() < deadline:
        done, _ = os.waitpid(pid, os.WNOHANG)
        if done:
            break
        drain()
        time.sleep(0.02)
    else:
        print("vtee did not exit after the signal", file=sys.stderr)
        sys.exit(1)

    def restored():
        drain()
        return lflag(master) & cooked == cooked

    wait_for(restored, f"the terminal to be restored after SIG{sig_name}")
    print("OK")


main()
