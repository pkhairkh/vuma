#!/usr/bin/env python3
"""SSH command runner using paramiko (no ssh binary needed).

Usage:
  python3 /home/z/my-project/scripts/remote.py "<command>"
  python3 /home/z/my-project/scripts/remote.py --file <local_script_path>
  python3 /home/z/my-project/scripts/remote.py --put <local_path> <remote_path>
  python3 /home/z/my-project/scripts/remote.py --get <remote_path> <local_path>

Env vars:
  REMOTE_TIMEOUT  default 3600 (seconds)
  REMOTE_HOST     default 155.138.203.27
  REMOTE_USER     default root
  REMOTE_PASS     default from script

Prints stdout+stderr to console. Exit code matches remote command's exit code.
"""
import sys
import os
import paramiko
import stat

HOST = os.environ.get("REMOTE_HOST", "155.138.203.27")
USER = os.environ.get("REMOTE_USER", "root")
PASS = os.environ.get("REMOTE_PASS", "gF_6@wQZqrU!Beud")
DEFAULT_TIMEOUT = int(os.environ.get("REMOTE_TIMEOUT", "3600"))

def _connect() -> paramiko.SSHClient:
    client = paramiko.SSHClient()
    client.set_missing_host_key_policy(paramiko.AutoAddPolicy())
    client.connect(HOST, username=USER, password=PASS, timeout=20, look_for_keys=False, allow_agent=False)
    return client

def run_remote(cmd: str, timeout: int = DEFAULT_TIMEOUT) -> int:
    client = _connect()
    try:
        # Use get_pty so that sudo prompts etc. work, but we don't need the password
        stdin, stdout, stderr = client.exec_command(cmd, timeout=timeout, get_pty=False)
        # Stream output
        chan = stdout.channel
        while not chan.exit_status_ready():
            if chan.recv_ready():
                data = chan.recv(65536).decode("utf-8", errors="replace")
                sys.stdout.write(data)
                sys.stdout.flush()
            if chan.recv_stderr_ready():
                data = chan.recv_stderr(65536).decode("utf-8", errors="replace")
                sys.stderr.write(data)
                sys.stderr.flush()
        # Drain any remaining
        while chan.recv_ready():
            sys.stdout.write(chan.recv(65536).decode("utf-8", errors="replace"))
        while chan.recv_stderr_ready():
            sys.stderr.write(chan.recv_stderr(65536).decode("utf-8", errors="replace"))
        rc = chan.recv_exit_status()
        return rc
    finally:
        client.close()

def run_remote_script(local_path: str, timeout: int = DEFAULT_TIMEOUT) -> int:
    with open(local_path, "r") as f:
        script = f.read()
    import base64
    b64 = base64.b64encode(script.encode()).decode()
    cmd = f"echo {b64} | base64 -d | bash"
    return run_remote(cmd, timeout=timeout)

def put_file(local_path: str, remote_path: str) -> int:
    client = _connect()
    try:
        sftp = client.open_sftp()
        sftp.put(local_path, remote_path)
        sftp.close()
        print(f"[put] {local_path} -> {HOST}:{remote_path}")
        return 0
    finally:
        client.close()

def get_file(remote_path: str, local_path: str) -> int:
    client = _connect()
    try:
        sftp = client.open_sftp()
        sftp.get(remote_path, local_path)
        sftp.close()
        print(f"[get] {HOST}:{remote_path} -> {local_path}")
        return 0
    finally:
        client.close()

if __name__ == "__main__":
    args = sys.argv[1:]
    if not args:
        print(__doc__, file=sys.stderr)
        sys.exit(2)
    if args[0] == "--file":
        sys.exit(run_remote_script(args[1]))
    elif args[0] == "--put":
        sys.exit(put_file(args[1], args[2]))
    elif args[0] == "--get":
        sys.exit(get_file(args[1], args[2]))
    else:
        sys.exit(run_remote(" ".join(args)))
