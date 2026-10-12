#!/usr/bin/env node
// Uso: ask-agent <agente> "mensagem"     (ou "-" para ler a mensagem do stdin)
//      ask-agent --list
const http = require('http');

const { JWIC_PORT: port, JWIC_TOKEN: token, JWIC_AGENT: from = 'shell', JWIC_DEPTH: depth = '0' } = process.env;
if (!port || !token) {
  console.error('ask-agent: rode este comando de dentro de um terminal do CommandDeck.');
  process.exit(2);
}

function request(method, route, body) {
  return new Promise((resolve, reject) => {
    const req = http.request(
      { host: '127.0.0.1', port, path: route, method, headers: { 'x-jwic-token': token, 'content-type': 'application/json' } },
      (res) => {
        let data = '';
        res.setEncoding('utf8');
        res.on('data', (d) => (data += d));
        res.on('end', () => {
          try { resolve({ status: res.statusCode, json: JSON.parse(data) }); } catch (e) { reject(e); }
        });
      },
    );
    req.on('error', reject);
    req.setTimeout(0);
    if (body) req.write(JSON.stringify(body));
    req.end();
  });
}

async function readStdin() {
  let s = '';
  for await (const chunk of process.stdin) s += chunk;
  return s;
}

(async () => {
  const [to, ...rest] = process.argv.slice(2);
  if (!to || to === '-h' || to === '--help') {
    console.log('Uso: ask-agent <agente> "mensagem"   |   ask-agent --list');
    process.exit(to ? 0 : 2);
  }
  if (to === '--list') {
    const { json } = await request('GET', '/agents');
    console.log(json.agents.join('\n'));
    return;
  }
  let message = rest.join(' ');
  if (!message || message === '-') message = await readStdin();
  if (!message.trim()) { console.error('ask-agent: mensagem vazia.'); process.exit(2); }

  const { status, json } = await request('POST', '/ask', { from, to, message, depth: Number(depth) });
  if (status !== 200) { console.error(`ask-agent: ${json.error}`); process.exit(1); }
  console.log(json.answer);
})().catch((e) => { console.error(`ask-agent: ${e.message}`); process.exit(1); });
