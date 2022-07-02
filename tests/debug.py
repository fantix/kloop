import asyncio
import ssl
import socket
import threading
import time
import pathlib

import kloop


def server():
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    cur = pathlib.Path(__file__).parent
    ctx.load_cert_chain(cur / "cert.pem", cur / "key.pem")

    ss = socket.socket()
    ss.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    ss.bind(("localhost", 8088))
    ss.listen(1)
    s, addr = ss.accept()
    s = ctx.wrap_socket(s, server_side=True)
    print(s)
    while True:
        req = s.recv(65536)
        print("<<<", req)
        if req == b'Hello':
            s.send(b'world\n')
        elif req.startswith(b'Sleep'):
            time.sleep(float(req.split()[1]))
            s.send(b'Sleep done\n')
        elif req == b'Bye':
            s.send(b'So long!\n')
            s.close()
            break
        else:
            s.send(b'unknown command\n')


async def main():
    ctx = ssl.create_default_context(cafile="tests/cert.pem")
    r, w = await asyncio.open_connection("localhost", 8088, ssl=ctx)
    print(r, w)
    w.write(b'Sleep 3')
    print(await r.readline())
    w.write(b'Hello')
    print(await r.readline())


t = threading.Thread(target=server)
t.daemon = True
t.start()

asyncio.set_event_loop_policy(kloop.KLoopPolicy())
asyncio.run(main())
