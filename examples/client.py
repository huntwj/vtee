#!/usr/bin/env python3
"""Drive an vtee session over its JSON socket, using only the standard library.

usage: client.py SOCKET SEND WAIT_FOR

Types SEND followed by Enter, waits up to 5 seconds for WAIT_FOR to appear on the screen, then prints the screen.
Start the session first, with write access:  vtee --headless --name demo --writable -- bash --norc
"""
import json
import socket
import sys

sock_path, send_text, wait_text = sys.argv[1:4]

conn = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
conn.connect(sock_path)
commands = [
    {"cmd": "send", "text": send_text + "\r"},
    {"cmd": "wait_for", "text": wait_text, "timeout_ms": 5000},
]
conn.sendall("".join(json.dumps(c) + "\n" for c in commands).encode())
conn.shutdown(socket.SHUT_WR)  # tell the server we are done sending

with conn.makefile() as replies:
    for line in replies:
        reply = json.loads(line)
        if reply["cmd"] == "wait_for":
            print("matched:", reply["matched"])
            for row in reply["snapshot"]["lines"]:
                if row.strip():
                    print(row.rstrip())
        elif reply["cmd"] == "error":
            sys.exit("vtee error: " + reply["message"])
