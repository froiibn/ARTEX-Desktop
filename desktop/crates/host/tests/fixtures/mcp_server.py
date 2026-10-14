import json,sys,time
for line in sys.stdin:
    message=json.loads(line)
    if 'id' not in message: continue
    method=message.get('method')
    if method=='initialize': result={'protocolVersion':'2024-11-05','capabilities':{'tools':{}},'serverInfo':{'name':'fixture','version':'1'}}
    elif method=='tools/list': result={'tools':[{'name':'echo','description':'Actual fixture echo','inputSchema':{'type':'object'}}]}
    elif method=='tools/call':
        if message['params'].get('arguments',{}).get('delay'): time.sleep(30)
        result={'content':[{'type':'text','text':message['params'].get('arguments',{}).get('text','')}],'isError':False}
    else: result={}
    print(json.dumps({'jsonrpc':'2.0','id':message['id'],'result':result}),flush=True)
