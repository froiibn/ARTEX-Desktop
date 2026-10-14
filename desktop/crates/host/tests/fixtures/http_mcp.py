import json,queue,time
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
from socketserver import TCPServer

class LoopbackHTTPServer(ThreadingHTTPServer):
    def server_bind(self):
        # HTTPServer.server_bind calls socket.getfqdn, which can stall in a
        # runner's DNS resolver before it publishes readiness. This numeric
        # loopback fixture needs neither a machine name nor reverse DNS.
        TCPServer.server_bind(self)
        self.server_name,self.server_port=self.server_address[:2]
events=queue.Queue(maxsize=32)
class Handler(BaseHTTPRequestHandler):
    protocol_version='HTTP/1.1'
    def log_message(self,*args): pass
    def respond(self,status,body=b'',media='application/json'):
        self.send_response(status);self.send_header('Content-Type',media);self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body);self.wfile.flush()
    def do_GET(self):
        if self.path.startswith('/sse'):
            self.send_response(200);self.send_header('Content-Type','text/event-stream');self.end_headers();self.wfile.write(b'event: endpoint\ndata: /messages\n\n');self.wfile.flush()
            try:
                while True:
                    try: data=events.get(timeout=15)
                    except queue.Empty: self.wfile.write(b': heartbeat\n\n');self.wfile.flush();continue
                    self.wfile.write(b'data: '+json.dumps(data).encode()+b'\n\n');self.wfile.flush()
            except (ConnectionError,OSError): pass
        else: self.respond(200,b'<h1>Actual local HTTP fixture</h1>','text/html')
    def do_POST(self):
        message=json.loads(self.rfile.read(int(self.headers['Content-Length'])))
        if 'id' not in message: self.respond(202);return
        method=message.get('method')
        if method=='initialize': result={'protocolVersion':'2024-11-05','capabilities':{'tools':{}},'serverInfo':{'name':'fixture','version':'1'}}
        elif method=='tools/list': result={'tools':[{'name':'echo','description':'Actual network fixture','inputSchema':{'type':'object'}}]}
        else:
            arguments=message.get('params',{}).get('arguments',{})
            if arguments.get('delay'):
                print('DELAY_ACCEPTED',flush=True)
                time.sleep(20)
            result={'content':[{'type':'text','text':arguments.get('text','')}],'isError':False}
        frame={'jsonrpc':'2.0','id':message['id'],'result':result}
        if self.path=='/messages': events.put(frame);self.respond(202)
        else: self.respond(200,json.dumps(frame).encode())
print('FIXTURE_START',flush=True)
server=LoopbackHTTPServer(('127.0.0.1',0),Handler)
print(server.server_port,flush=True)
server.serve_forever()
