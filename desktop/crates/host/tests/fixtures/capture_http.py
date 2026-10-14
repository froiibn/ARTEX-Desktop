import http.server, ssl, sys
from socketserver import TCPServer

class LoopbackHTTPServer(http.server.ThreadingHTTPServer):
    def server_bind(self):
        # Readiness for numeric loopback must not depend on reverse DNS. Keep
        # the real HTTP/TLS socket, while omitting HTTPServer's getfqdn call.
        TCPServer.server_bind(self)
        self.server_name,self.server_port=self.server_address[:2]
class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version='HTTP/1.1'
    def do_GET(self):
        if self.headers.get('Upgrade'):
            self.send_response(101); self.send_header('Connection','Upgrade'); self.send_header('Upgrade',self.headers['Upgrade']); self.end_headers()
            while True:
                data=self.rfile.read(5)
                if not data: return
                self.wfile.write(data); self.wfile.flush()
        body=b'actual captured response'
        self.send_response(200); self.send_header('Content-Type','text/plain'); self.send_header('Content-Length',str(len(body))); self.end_headers(); self.wfile.write(body)
    def do_POST(self):
        body=self.rfile.read(int(self.headers.get('Content-Length','0')))
        self.send_response(201); self.send_header('Content-Length',str(len(body))); self.end_headers(); self.wfile.write(body)
    def log_message(self,*args): pass
print('FIXTURE_START',flush=True)
server=LoopbackHTTPServer(('127.0.0.1',0),Handler)
if len(sys.argv)>1:
    context=ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER); context.load_cert_chain(sys.argv[1],sys.argv[2]); server.socket=context.wrap_socket(server.socket,server_side=True)
print(server.server_port,flush=True)
server.serve_forever()
