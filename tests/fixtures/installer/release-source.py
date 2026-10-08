"""A release source on loopback, for the cells that stage `crucible update`.

It names one release as the newest and serves that release's files from one
directory, at the two paths `crucible update` asks a release source for:
`/releases/latest`, and `/releases/download/v<version>/<name>`. Anything else
is answered 404. Every request line it hears, whatever its method, is added to
a log, so a cell can show what was asked, or that nothing was.

Usage: release-source.py PORT_FILE LOG_FILE VERSION DIRECTORY

The port it listens on is written to PORT_FILE once it is listening.
"""

import http.server
import json
import os
import socketserver
import sys

port_file, log_file, version, directory = sys.argv[1:5]
served = {name: os.path.join(directory, name) for name in os.listdir(directory)}
download = f"/releases/download/v{version}/"


class Source(http.server.BaseHTTPRequestHandler):
    def parse_request(self):
        parsed = super().parse_request()
        if self.requestline:
            with open(log_file, "a") as log:
                log.write(self.requestline + "\n")
        return parsed

    def do_GET(self):
        name = self.path[len(download):] if self.path.startswith(download) else None
        if self.path == "/releases/latest":
            body = json.dumps({"tag_name": f"v{version}"}).encode()
        elif name in served:
            with open(served[name], "rb") as file:
                body = file.read()
        else:
            self.send_error(404)
            return
        self.send_response(200)
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


# A plain TCP server, because HTTPServer looks up the name of the address it
# binds, and on a macOS runner that lookup outlasts the wait for the port.
server = socketserver.TCPServer(("127.0.0.1", 0), Source)
open(log_file, "a").close()
with open(port_file, "w") as port:
    port.write(str(server.server_address[1]))
server.serve_forever()
