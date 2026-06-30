#!/usr/bin/env python3
"""Minimal Source RCON client — sends commands to the CS2 server for the Slice 0 gate."""
import socket, struct, sys, time

HOST, PORT, PW = "127.0.0.1", 27015, "s2script"

def pkt(pid, ptype, body):
    data = struct.pack("<ii", pid, ptype) + body.encode() + b"\x00\x00"
    return struct.pack("<i", len(data)) + data

def recv_pkt(s):
    raw = b""
    while len(raw) < 4:
        c = s.recv(4 - len(raw))
        if not c: return None
        raw += c
    size = struct.unpack("<i", raw)[0]
    data = b""
    while len(data) < size:
        c = s.recv(size - len(data))
        if not c: break
        data += c
    pid, ptype = struct.unpack("<ii", data[:8])
    return pid, ptype, data[8:-2].decode("utf-8", "replace")

def main():
    cmds = sys.argv[1:]
    s = socket.create_connection((HOST, PORT), timeout=10)
    s.sendall(pkt(1, 3, PW))            # SERVERDATA_AUTH
    s.settimeout(3)
    try:
        while True:
            r = recv_pkt(s)
            if r is None: break
            if r[1] == 2:                # auth response
                if r[0] == -1:
                    print("RCON AUTH FAILED"); sys.exit(1)
                break
    except socket.timeout:
        pass
    print("RCON connected.")
    for c in cmds:
        print(f"\n>>> {c}")
        s.sendall(pkt(2, 2, c))         # SERVERDATA_EXECCOMMAND
        time.sleep(0.6)
        out = ""
        s.settimeout(2)
        try:
            while True:
                r = recv_pkt(s)
                if r is None: break
                out += r[2]
        except socket.timeout:
            pass
        if out.strip():
            print(out.strip())
        time.sleep(0.8)
    s.close()

if __name__ == "__main__":
    main()
