"""A model on loopback that asks for one command and keeps what came back.

It answers the Anthropic messages API with a stream. Asked first, it calls the
`bash` tool to run `echo staged-sandbox-ok`; asked again with that call's
result, it writes the result to RESULT_FILE and ends the turn. Nothing it is
sent leaves this machine.

Usage: loopback-model.py PORT_FILE RESULT_FILE

The port it listens on is written to PORT_FILE once it is listening.
"""

import http.server
import json
import sys

port_file, result_file = sys.argv[1:3]
MARK = "staged-sandbox-ok"


def stream(blocks, stop):
    events = [{"type": "message_start", "message": {"id": "m", "usage": {"input_tokens": 1, "output_tokens": 0}}}]
    for index, block in enumerate(blocks):
        if block["type"] == "tool_use":
            events.append({"type": "content_block_start", "index": index, "content_block": dict(block, input={})})
            events.append({"type": "content_block_delta", "index": index,
                           "delta": {"type": "input_json_delta", "partial_json": json.dumps(block["input"])}})
        else:
            events.append({"type": "content_block_start", "index": index, "content_block": block})
        events.append({"type": "content_block_stop", "index": index})
    events.append({"type": "message_delta", "delta": {"stop_reason": stop}, "usage": {"output_tokens": 1}})
    events.append({"type": "message_stop"})
    return "".join(f"event: {e['type']}\ndata: {json.dumps(e)}\n\n" for e in events).encode()


class Model(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["content-length"])))
        results = [part for message in body.get("messages", []) if isinstance(message.get("content"), list)
                   for part in message["content"] if part.get("type") == "tool_result"]
        if results:
            with open(result_file, "w") as kept:
                json.dump(results[-1], kept)
            answer = stream([{"type": "text", "text": "done"}], "end_turn")
        else:
            answer = stream([{"type": "tool_use", "id": "call-1", "name": "bash",
                              "input": {"command": f"echo {MARK}"}}], "tool_use")
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("content-length", str(len(answer)))
        self.end_headers()
        self.wfile.write(answer)

    def log_message(self, *args):
        pass


server = http.server.HTTPServer(("127.0.0.1", 0), Model)
with open(port_file, "w") as port:
    port.write(str(server.server_address[1]))
server.serve_forever()
