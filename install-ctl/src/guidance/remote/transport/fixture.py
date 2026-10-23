"""Loopback-only smart Git HTTP, SSH, and synthetic credential fixtures."""

import http.server
import json
import os
import shlex
import subprocess
import sys
import time
import urllib.parse


def append(path, record):
    with open(path, "a", encoding="utf-8") as output:
        output.write(json.dumps(record) + "\n")


def serve(root, log, authentication):
    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def do_GET(self):
            self.respond()

        def do_POST(self):
            self.respond()

        def respond(self):
            authorized = bool(self.headers.get("Authorization"))
            if self.headers.get("Transfer-Encoding", "").lower() == "chunked":
                chunks = []
                while True:
                    length = int(self.rfile.readline().split(b";", 1)[0], 16)
                    if not length:
                        self.rfile.readline()
                        break
                    chunks.append(self.rfile.read(length))
                    self.rfile.read(2)
                body = b"".join(chunks)
            else:
                body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
            append(log, {
                "method": self.command,
                "path": self.path,
                "authorized": authorized,
                "fixture_header": self.headers.get("X-Transport-Fixture"),
                "second_header": self.headers.get("X-Transport-Second"),
                "body": body.decode("ascii", errors="replace"),
            })
            if authentication == "mutable-access" and os.path.exists(os.path.join(root, "deny-access")):
                self.send_response(403)
                self.send_header("Content-Length", "0")
                self.end_headers()
                return
            if authentication == "stall-handshake" or (
                authentication == "stall-fetch" and b"command=fetch" in body
            ):
                time.sleep(30)
                return
            if authentication == "auth" and not authorized:
                self.send_response(401)
                self.send_header("WWW-Authenticate", 'Basic realm="fixture"')
                self.send_header("Content-Length", "0")
                self.end_headers()
                return
            path = urllib.parse.urlsplit(self.path)
            environment = os.environ.copy()
            environment.update({
                "GIT_PROJECT_ROOT": root,
                "GIT_HTTP_EXPORT_ALL": "1",
                "REQUEST_METHOD": self.command,
                "PATH_INFO": path.path,
                "QUERY_STRING": path.query,
                "CONTENT_TYPE": self.headers.get("Content-Type", ""),
                "CONTENT_LENGTH": str(len(body)),
                "SERVER_PROTOCOL": "HTTP/1.1",
            })
            if self.headers.get("Git-Protocol"):
                environment["HTTP_GIT_PROTOCOL"] = self.headers["Git-Protocol"]
            completed = subprocess.run(
                ["git", "http-backend"], input=body, env=environment,
                stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False,
            )
            response = completed.stdout
            if completed.returncode:
                append(log, {"cgi_exit": completed.returncode})
            if b"\r\n\r\n" not in response:
                self.send_error(500, "Git HTTP backend did not produce a CGI response")
                return
            headers, payload = response.split(b"\r\n\r\n", 1)
            fields = [line.decode("ascii").split(":", 1) for line in headers.split(b"\r\n")]
            status = next((int(value.strip().split()[0]) for key, value in fields if key.lower() == "status"), 200)
            self.send_response(status)
            for key, value in fields:
                if key.lower() != "status":
                    self.send_header(key, value.strip())
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    print(server.server_port, flush=True)
    server.serve_forever()


if __name__ == "__main__":
    if sys.argv[1] == "http":
        serve(*sys.argv[2:])
    elif sys.argv[1] == "credential":
        sys.stdin.buffer.read()
        if len(sys.argv) > 3:
            append(sys.argv[2], {"action": sys.argv[-1]})
        if len(sys.argv) > 2 and sys.argv[-1] == "get":
            print("username=fixture-user\npassword=fixture-password\n")
    elif sys.argv[1] == "ssh":
        log, *arguments = sys.argv[2:]
        append(log, {"arguments": arguments})
        command = " ".join(arguments[-2:]) if arguments[-2] == "git-upload-pack" else arguments[-1]
        if not command.startswith("git-upload-pack "):
            raise SystemExit("fixture only permits git-upload-pack")
        path = shlex.split(command)[1]
        if len(path) > 3 and path[0] == "/" and path[2] == ":":
            path = path[1:]
        raise SystemExit(subprocess.call(["git", "upload-pack", path]))
    else:
        raise SystemExit("unknown fixture mode")
