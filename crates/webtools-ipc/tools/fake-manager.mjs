// Fake manager used only for protocol tests. Not the real Electron app.
import net from 'node:net'

const name = process.argv[2] ?? ('WT.Fake.' + Math.random().toString(16).slice(2))
const state = { remembered: [] }

function frame (envelope) {
  const body = Buffer.from(JSON.stringify(envelope), 'utf8')
  const head = Buffer.allocUnsafe(4)
  head.writeUInt32LE(body.length, 0)
  return Buffer.concat([head, body])
}

net.createServer((socket) => {
  console.error('connection')
  let buffer = Buffer.alloc(0)
  socket.on('data', (chunk) => {
    buffer = Buffer.concat([buffer, chunk])
    while (buffer.length >= 4) {
      const length = buffer.readUInt32LE(0)
      if (buffer.length - 4 < length) break
      const message = JSON.parse(buffer.toString('utf8', 4, 4 + length))
      buffer = buffer.subarray(4 + length)
      console.error('recv', message.type)
      handle(socket, message)
    }
  })
}).listen('\\\\.\\pipe\\' + name, () => {
  console.log(JSON.stringify({ pipe: name, listening: true }))
})

function handle (socket, message) {
  const reply = (ok, result, error) => { console.error('reply', message.type, ok); socket.write(frame({
    protocolVersion: 1,
    requestId: message.requestId,
    type: 'ack',
    payload: {
      replyToId: message.requestId,
      replyToType: message.type,
      ok,
      result,
      error: error ? { code: error, message: 'fail' } : null,
    },
  })) }

  switch (message.type) {
    case 'electron-host-ready':
      return reply(true, { accepted: true, capabilities: ['app-memory', 'feature-invoke'] })
    case 'app-memory-get':
      return reply(true, { items: state.remembered })
    case 'app-memory-remember': {
      state.remembered.push(message.payload)
      return reply(true, { remembered: state.remembered.length })
    }
    default:
      return reply(false, null, 'UNSUPPORTED')
  }
}