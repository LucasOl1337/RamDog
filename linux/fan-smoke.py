#!/usr/bin/env python3
"""Short hardware test: stabilize only populated fan headers, then restore BIOS.

Run from the repository with: sudo -v && python3 linux/fan-smoke.py
Requires an NCT6687-compatible hwmon driver. Never holds a fan override past exit.
"""
import json
import os
import socket
import subprocess
import time

BINARY = os.path.abspath('target/release/ramdog')


def request(command):
    with socket.socket(socket.AF_UNIX) as conn:
        conn.settimeout(2)
        conn.connect('\0ramdog-fans-%d-%d' % (os.getuid(), os.getpid()))
        conn.sendall((command + '\n').encode())
        conn.shutdown(socket.SHUT_WR)
        data = bytearray()
        while chunk := conn.recv(65536):
            data.extend(chunk)
        return json.loads(data)


original = {}
for i in range(1, 9):
    prefix = '/sys/class/hwmon/hwmon7/pwm%d' % i
    if os.path.exists(prefix + '_enable'):
        original[i] = (open(prefix + '_enable').read().strip(), open(prefix).read().strip())
helper = subprocess.Popen(['sudo', '-n', BINARY, '--fan-helper', str(os.getpid())])
try:
    for _ in range(30):
        try:
            start = request('status')
            break
        except OSError:
            if helper.poll() is not None:
                raise RuntimeError('helper exited before socket ready')
            time.sleep(0.1)
    else:
        raise RuntimeError('helper socket timeout')
    assert start['fans'], start
    print('before', [(f['name'], f['auto'], f['rpm']) for f in start['fans']])
    request('stab on')
    time.sleep(2)
    held = request('status')
    print('held', [(f['name'], f['auto'], f['rpm'], f['pct']) for f in held['fans']])
    assert held['stab']['on'] and held['error'] is None, held
    for fan in held['fans']:
        name = fan['name'].lower()
        if 'pump' in name or (fan['rpm'] or 0) == 0:
            assert fan['auto'], fan
        else:
            assert not fan['auto'], fan
    print('PASS: populated fans controlled, pump and unused headers kept on BIOS')
finally:
    if helper.poll() is None:
        try:
            request('stab off')
        finally:
            helper.terminate()
    helper.wait(timeout=5)
    for i, (mode, pwm) in original.items():
        path = '/sys/class/hwmon/hwmon7/pwm%d' % i
        now = open(path + '_enable').read().strip()
        assert now == mode, (i, mode, now)
    print('PASS: all original fan modes restored')
