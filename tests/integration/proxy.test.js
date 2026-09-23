// 端到端集成测试：mock 两个协议的上游，验证识别→查表→改写→转发→透传全链路与各错误场景
'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const http = require('http');
const { createLocalProxy } = require('../../electron/services/proxy/index');

// 起 mock 上游用的小工具
function startMockServer(handler) {
  return new Promise((resolve) => {
    const server = http.createServer(handler);
    server.listen(0, '127.0.0.1', () => resolve({ server, port: server.address().port }));
  });
}
function stopServer(server) {
  return new Promise((resolve) => server.close(() => resolve()));
}
// 发请求的小工具：返回 { status, headers, text }
function request(port, method, path, headers, body) {
  return new Promise((resolve, reject) => {
    const req = http.request({ hostname: '127.0.0.1', port, method, path, headers }, (res) => {
      const chunks = [];
      res.on('data', (c) => chunks.push(c));
      res.on('end', () => resolve({ status: res.statusCode, headers: res.headers, text: Buffer.concat(chunks).toString('utf8') }));
    });
    req.on('error', reject);
    if (body) req.write(body);
    req.end();
  });
}

// anthropic 风格的请求体 / 响应 SSE 流
function claudeBody(model, stream) {
  return Buffer.from(JSON.stringify({ model, max_tokens: 1024, system: 'be brief', stream: !!stream, messages: [{ role: 'user', content: 'hi' }] }));
}
function openaiBody(model) {
  return Buffer.from(JSON.stringify({ model, messages: [{ role: 'user', content: 'hi' }] }));
}

test('端到端：anthropic 协议识别、token 替换、model 改写、SSE 透传', async () => {
  // 1. mock anthropic 上游：记录收到的头与体，回一段 SSE
  let upstreamSeen = null;
  const mock = await startMockServer((req, res) => {
    const chunks = [];
    req.on('data', (c) => chunks.push(c));
    req.on('end', () => {
      upstreamSeen = { url: req.url, headers: req.headers, body: JSON.parse(Buffer.concat(chunks).toString('utf8')) };
      res.writeHead(200, { 'content-type': 'text/event-stream' });
      res.write('event: message_start\ndata: {"type":"message_start"}\n\n');
      res.write('event: message_stop\ndata: {"type":"message_stop"}\n\n');
      res.end();
    });
  });

  // 2. 代理配置：映射 claude-* → mock 供应商
  const config = {
    settings: { port: 0 },
    protocols: [{
      id: 'anthropic', name: 'Anthropic', enabled: true,
      providers: [{ id: 'p1', name: 'mock上游', enabled: true, baseUrl: `http://127.0.0.1:${mock.port}`, token: 'sk-real-token', models: ['glm-4.6'] }],
      mappings: [
        { id: 'm1', enabled: true, clientModel: 'claude-sonnet-4-*', providerId: 'p1', realModel: 'glm-4.6' },
        { id: 'm9', enabled: true, clientModel: '*', providerId: 'p1', realModel: 'glm-4.6' }
      ]
    }]
  };
  const proxy = createLocalProxy({ getConfig: () => config });
  const { port } = await proxy.start();

  // 3. 模拟 Claude Code 的请求：x-api-key 带假 key
  const resp = await request(port, 'POST', '/v1/messages', {
    'content-type': 'application/json',
    'x-api-key': 'sk-fake-from-client',
    'anthropic-version': '2023-06-01',
    'accept': 'text/event-stream'
  }, claudeBody('claude-sonnet-4-20250514', true));

  // 4. 断言：透传成功、上游收到的是真 token 与真实模型名、假 key 没有外泄
  assert.equal(resp.status, 200);
  assert.ok(resp.text.includes('message_start'));
  assert.equal(resp.headers['content-type'], 'text/event-stream');
  assert.equal(upstreamSeen.url, '/v1/messages');
  assert.equal(upstreamSeen.headers['x-api-key'], 'sk-real-token');
  assert.equal(upstreamSeen.headers.authorization, undefined);
  assert.equal(upstreamSeen.body.model, 'glm-4.6');
  assert.equal(upstreamSeen.headers['anthropic-version'], '2023-06-01');

  await proxy.stop();
  await stopServer(mock.server);
});

test('端到端：响应 model 回写为客户端模型名（非流式与 SSE 流式）', async () => {
  // mock 上游回什么模型名，客户端就应该看到自己请求的模型名
  const mock = await startMockServer((req, res) => {
    const chunks = [];
    req.on('data', (c) => chunks.push(c));
    req.on('end', () => {
      const wantStream = JSON.parse(Buffer.concat(chunks).toString('utf8')).stream === true;
      if (wantStream) {
        res.writeHead(200, { 'content-type': 'text/event-stream' });
        // 故意把 message_start 事件从 JSON 中间劈成两次 write，验证回写的事件拼装不依赖 chunk 边界
        res.write('event: message_start\ndata: {"type":"message_start","message":{"mo');
        res.write('del":"glm-4.6","role":"assistant"}}\n\n');
        res.write('event: message_stop\ndata: {"type":"message_stop"}\n\n');
        res.end();
      } else {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(JSON.stringify({ id: 'msg_1', type: 'message', model: 'glm-4.6', content: [] }));
      }
    });
  });

  const config = {
    settings: { port: 0 },
    protocols: [{
      id: 'anthropic', name: 'Anthropic', enabled: true,
      providers: [{ id: 'p1', name: 'mock上游', enabled: true, baseUrl: `http://127.0.0.1:${mock.port}`, token: 'sk-real-token', models: ['glm-4.6'] }],
      mappings: [{ id: 'm1', enabled: true, clientModel: 'cnsinda-glm-4.6', providerId: 'p1', realModel: 'glm-4.6' }]
    }]
  };
  const proxy = createLocalProxy({ getConfig: () => config });
  const { port } = await proxy.start();

  // 非流式：响应 JSON 顶层 model 已是客户端模型名，content-length 与改写后字节数一致
  const nonStream = await request(port, 'POST', '/v1/messages', {
    'content-type': 'application/json',
    'x-api-key': 'sk-fake-from-client',
    'anthropic-version': '2023-06-01'
  }, claudeBody('cnsinda-glm-4.6', false));
  assert.equal(nonStream.status, 200);
  assert.equal(JSON.parse(nonStream.text).model, 'cnsinda-glm-4.6');
  assert.equal(Number(nonStream.headers['content-length']), Buffer.byteLength(nonStream.text));

  // 流式：message_start 里的 model 已回写，其余事件原样
  const stream = await request(port, 'POST', '/v1/messages', {
    'content-type': 'application/json',
    'x-api-key': 'sk-fake-from-client',
    'anthropic-version': '2023-06-01',
    'accept': 'text/event-stream'
  }, claudeBody('cnsinda-glm-4.6', true));
  assert.equal(stream.status, 200);
  const startLine = stream.text.split('\n').find((l) => l.startsWith('data:') && l.includes('message_start'));
  assert.equal(JSON.parse(startLine.slice(5)).message.model, 'cnsinda-glm-4.6');
  assert.ok(stream.text.includes('event: message_stop'));

  await proxy.stop();
  await stopServer(mock.server);
});

test('端到端：openai 协议走 Authorization Bearer', async () => {
  let upstreamSeen = null;
  const mock = await startMockServer((req, res) => {
    const chunks = [];
    req.on('data', (c) => chunks.push(c));
    req.on('end', () => {
      upstreamSeen = { headers: req.headers, body: JSON.parse(Buffer.concat(chunks).toString('utf8')) };
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ id: 'x', object: 'chat.completion', usage: { prompt_tokens: 3, completion_tokens: 5 } }));
    });
  });

  const config = {
    settings: { port: 0 },
    protocols: [{
      id: 'openai', name: 'OpenAI', enabled: true,
      providers: [{ id: 'q1', name: 'mock openai', enabled: true, baseUrl: `http://127.0.0.1:${mock.port}/v1`, token: 'sk-openai-real', models: ['deepseek-chat'] }],
      mappings: [{ id: 'm1', enabled: true, clientModel: 'gpt-4o', providerId: 'q1', realModel: 'deepseek-chat' }]
    }]
  };
  const proxy = createLocalProxy({ getConfig: () => config });
  const { port } = await proxy.start();

  const resp = await request(port, 'POST', '/v1/chat/completions', {
    'content-type': 'application/json',
    authorization: 'Bearer sk-fake'
  }, openaiBody('gpt-4o'));

  assert.equal(resp.status, 200);
  assert.equal(upstreamSeen.headers.authorization, 'Bearer sk-openai-real');
  assert.equal(upstreamSeen.headers['x-api-key'], undefined);
  assert.equal(upstreamSeen.body.model, 'deepseek-chat');

  await proxy.stop();
  await stopServer(mock.server);
});

test('端到端：映射未命中返回 404 且文案明确', async () => {
  const config = {
    settings: { port: 0 },
    protocols: [{
      id: 'openai', name: 'OpenAI', enabled: true,
      providers: [{ id: 'q1', name: 'mock', enabled: true, baseUrl: 'http://127.0.0.1:9', token: 't', models: [] }],
      mappings: [{ id: 'm1', enabled: true, clientModel: 'gpt-4o', providerId: 'q1', realModel: 'x' }]
    }]
  };
  const proxy = createLocalProxy({ getConfig: () => config });
  const { port } = await proxy.start();

  const resp = await request(port, 'POST', '/v1/chat/completions', { 'content-type': 'application/json' }, openaiBody('gpt-5-turbo'));
  assert.equal(resp.status, 404);
  assert.ok(resp.text.includes('gpt-5-turbo'));

  await proxy.stop();
});

test('端到端：无法识别协议返回 400 且附得分明细', async () => {
  const config = { settings: { port: 0 }, protocols: [] };
  const proxy = createLocalProxy({ getConfig: () => config });
  const { port } = await proxy.start();

  const resp = await request(port, 'POST', '/foo', { 'content-type': 'application/json' }, Buffer.from('{}'));
  assert.equal(resp.status, 400);
  const parsed = JSON.parse(resp.text);
  assert.ok(parsed.error.detail.scores);

  await proxy.stop();
});

test('端到端：GET /v1/models 由代理接管，anthropic 头返回激活映射且不透传上游', async () => {
  // mock 上游：若被请求即失败（模型列表不应透传）
  let upstreamHit = false;
  const mock = await startMockServer((req, res) => {
    upstreamHit = true;
    res.writeHead(500).end();
  });

  const config = {
    settings: { port: 0 },
    protocols: [{
      id: 'anthropic', name: 'Anthropic', enabled: true,
      providers: [{ id: 'p1', name: 'mock上游', enabled: true, baseUrl: `http://127.0.0.1:${mock.port}`, token: 'sk-real', models: ['glm-4.6'] }],
      mappings: [
        { id: 'm1', enabled: true, clientModel: 'claude-opus-4', providerId: 'p1', realModel: 'glm-4.6' },
        { id: 'm2', enabled: false, clientModel: 'claude-haiku-4', providerId: 'p1', realModel: 'glm-4.5' },
        { id: 'm3', enabled: true, clientModel: 'claude-sonnet-4-*', providerId: 'p1', realModel: 'glm-4.6' },
        { id: 'm9', enabled: true, clientModel: '*', providerId: 'p1', realModel: 'glm-4.6' }
      ]
    }]
  };
  const proxy = createLocalProxy({ getConfig: () => config });
  const { port } = await proxy.start();

  const resp = await request(port, 'GET', '/v1/models', { 'anthropic-version': '2023-06-01', 'x-api-key': 'sk-fake' });
  assert.equal(resp.status, 200);
  const parsed = JSON.parse(resp.text);
  // 只返回激活映射的模型名；停用映射与 * 兜底不出现
  assert.deepEqual(parsed.data.map((m) => m.id), ['claude-opus-4']);
  assert.equal(parsed.has_more, false);
  assert.equal(upstreamHit, false, '模型列表请求不应透传上游');

  await proxy.stop();
  await stopServer(mock.server);
});

test('端到端：GET /v1/models 按请求头区分协议，openai 客户端拿到 openai 格式', async () => {
  const config = {
    settings: { port: 0 },
    protocols: [{
      id: 'openai', name: 'OpenAI', enabled: true,
      providers: [{ id: 'q1', name: 'mock', enabled: true, baseUrl: 'http://127.0.0.1:9', token: 't', models: ['deepseek-chat'] }],
      mappings: [
        { id: 'm1', enabled: true, clientModel: 'gpt-4o', providerId: 'q1', realModel: 'deepseek-chat' },
        { id: 'm2', enabled: true, clientModel: 'gpt-4o-mini', providerId: 'q1', realModel: 'deepseek-chat' }
      ]
    }]
  };
  const proxy = createLocalProxy({ getConfig: () => config });
  const { port } = await proxy.start();

  const resp = await request(port, 'GET', '/v1/models', { authorization: 'Bearer sk-fake' });
  assert.equal(resp.status, 200);
  const parsed = JSON.parse(resp.text);
  assert.equal(parsed.object, 'list');
  assert.deepEqual(parsed.data.map((m) => m.id), ['gpt-4o', 'gpt-4o-mini']);
  assert.ok(parsed.data.every((m) => m.object === 'model'));

  // 单模型查询：命中返回单项，未命中 404
  const hit = await request(port, 'GET', '/v1/models/gpt-4o', { authorization: 'Bearer sk-fake' });
  assert.equal(hit.status, 200);
  assert.equal(JSON.parse(hit.text).id, 'gpt-4o');
  const miss = await request(port, 'GET', '/v1/models/no-such', { authorization: 'Bearer sk-fake' });
  assert.equal(miss.status, 404);
  assert.equal(JSON.parse(miss.text).error.code, 'model_not_found');

  await proxy.stop();
});

test('端到端：上游不可达返回 502', async () => {
  const config = {
    settings: { port: 0 },
    protocols: [{
      id: 'openai', name: 'OpenAI', enabled: true,
      providers: [{ id: 'q1', name: 'dead', enabled: true, baseUrl: 'http://127.0.0.1:1', token: 't', models: [] }],
      mappings: [{ id: 'm1', enabled: true, clientModel: 'gpt-4o', providerId: 'q1', realModel: 'x' }]
    }]
  };
  const proxy = createLocalProxy({ getConfig: () => config });
  const { port } = await proxy.start();

  const resp = await request(port, 'POST', '/v1/chat/completions', { 'content-type': 'application/json' }, openaiBody('gpt-4o'));
  assert.equal(resp.status, 502);

  await proxy.stop();
});

test('端到端：客户端提前断开时上游请求被中止', async () => {
  // mock 上游持续发 SSE；上游侧连接关闭时 resolve，用来断言代理确实中止了上游
  let upstreamClosed = null;
  const upstreamClosedPromise = new Promise((resolve) => { upstreamClosed = resolve; });
  const mock = await startMockServer((req, res) => {
    res.writeHead(200, { 'content-type': 'text/event-stream' });
    res.write('event: message_start\ndata: {"type":"message_start"}\n\n');
    const timer = setInterval(() => res.write('event: ping\ndata: {}\n\n'), 50);
    res.on('close', () => {
      clearInterval(timer);
      upstreamClosed();
    });
  });

  const config = {
    settings: { port: 0 },
    protocols: [{
      id: 'anthropic', name: 'Anthropic', enabled: true,
      providers: [{ id: 'p1', name: 'mock上游', enabled: true, baseUrl: `http://127.0.0.1:${mock.port}`, token: 'sk-real', models: ['glm-4.6'] }],
      mappings: [{ id: 'm9', enabled: true, clientModel: '*', providerId: 'p1', realModel: 'glm-4.6' }]
    }]
  };
  const proxy = createLocalProxy({ getConfig: () => config });
  const { port } = await proxy.start();

  // 客户端收到第一块数据后立即断开（模拟取消请求）
  await new Promise((resolve, reject) => {
    const req = http.request({
      hostname: '127.0.0.1', port, method: 'POST', path: '/v1/messages',
      headers: { 'content-type': 'application/json', 'anthropic-version': '2023-06-01' }
    }, (res) => {
      res.once('data', () => {
        req.destroy();
        resolve();
      });
    });
    req.on('error', () => {});
    req.end(claudeBody('claude-sonnet-4-20250514', true));
  });

  // 上游连接应在 2 秒内被代理中止，而不是一直流到自然结束
  await Promise.race([
    upstreamClosedPromise,
    new Promise((_, reject) => setTimeout(() => reject(new Error('客户端断开后上游连接未被中止')), 2000))
  ]);

  await proxy.stop();
  await stopServer(mock.server);
});

test('端到端：请求体超上限返回 413 JSON 而非连接重置', async () => {
  const config = {
    settings: { port: 0 },
    protocols: [{
      id: 'anthropic', name: 'Anthropic', enabled: true,
      providers: [{ id: 'p1', name: 'mock', enabled: true, baseUrl: 'http://127.0.0.1:9', token: 't', models: [] }],
      mappings: [{ id: 'm9', enabled: true, clientModel: '*', providerId: 'p1', realModel: 'x' }]
    }]
  };
  // 注入 1024 字节的小上限，发送 2048 字节请求体
  const proxy = createLocalProxy({ getConfig: () => config, maxBodyBytes: 1024 });
  const { port } = await proxy.start();

  const big = Buffer.alloc(2048, 97);
  const resp = await request(port, 'POST', '/v1/messages', { 'content-type': 'application/json', 'anthropic-version': '2023-06-01' }, big);
  assert.equal(resp.status, 413);
  const parsed = JSON.parse(resp.text);
  assert.ok(parsed.error);

  await proxy.stop();
});

test('并发 start：复用同一次启动，不会各自 listen 后互踩', async () => {
  const config = { settings: { port: 0 }, protocols: [] };
  const proxy = createLocalProxy({ getConfig: () => config });

  // 同一个 tick 连发三次，模拟托盘与界面几乎同时触发启动
  const results = await Promise.all([proxy.start(), proxy.start(), proxy.start()]);

  const ports = new Set(results.map((item) => item.port));
  assert.equal(ports.size, 1, '三次并发 start 应返回同一个端口');
  assert.equal(proxy.getStatus().running, true);
  assert.equal(proxy.getStatus().port, results[0].port);

  // 已在运行时应直接复用，不再新起监听
  const again = await proxy.start();
  assert.equal(again.port, results[0].port);

  await proxy.stop();
  assert.equal(proxy.getStatus().running, false);
});

test('并发 start（固定端口）：三次并发只起一个监听，stop 后端口真的释放', async () => {
  // 借一个当前空闲的端口当固定端口用
  const probe = await startMockServer(() => {});
  const fixedPort = probe.port;
  await stopServer(probe.server);

  const config = { settings: { port: fixedPort }, protocols: [] };
  const proxy = createLocalProxy({ getConfig: () => config });

  const results = await Promise.allSettled([proxy.start(), proxy.start(), proxy.start()]);
  assert.deepEqual(results.map((item) => item.status), ['fulfilled', 'fulfilled', 'fulfilled']);
  assert.deepEqual([...new Set(results.map((item) => item.value.port))], [fixedPort]);
  assert.equal(proxy.getStatus().running, true);

  await proxy.stop();
  assert.equal(proxy.getStatus().running, false);

  // 端口真的释放了才算过：旧写法下第一个 listen 成功的 server 会变成没人引用的孤儿，端口一直占着
  const recheck = http.createServer(() => {});
  await new Promise((resolve, reject) => {
    recheck.once('error', reject);
    recheck.listen(fixedPort, '127.0.0.1', resolve);
  });
  await stopServer(recheck);
});

test('start() 在途时 stop()：不会把随后落地的监听留在运行态', async () => {
  // 借一个当前空闲的端口当固定端口用
  const probe = await startMockServer(() => {});
  const fixedPort = probe.port;
  await stopServer(probe.server);

  const config = { settings: { port: fixedPort }, protocols: [] };
  const proxy = createLocalProxy({ getConfig: () => config });

  // 不 await start：模拟「启动」还没落地就点了「停止」
  const startPromise = proxy.start();
  await proxy.stop();
  await startPromise;

  // stop() 返回后必须真的已停止：旧写法下 stop() 立刻返回，随后 listen 落地就留在运行态
  assert.equal(proxy.getStatus().running, false, 'stop() 返回后不应仍在运行');

  // 端口真的释放了才算过：在途的 listen 若落地成孤儿，这里会 EADDRINUSE
  const recheck = http.createServer(() => {});
  await new Promise((resolve, reject) => {
    recheck.once('error', reject);
    recheck.listen(fixedPort, '127.0.0.1', resolve);
  });
  await stopServer(recheck);
});

test('start/stop/start 同一 tick 三连：第三次 start resolve 时是真的在监听', async () => {
  // 借一个当前空闲的端口当固定端口用
  const probe = await startMockServer(() => {});
  const fixedPort = probe.port;
  await stopServer(probe.server);

  const config = { settings: { port: fixedPort }, protocols: [] };
  const proxy = createLocalProxy({ getConfig: () => config });

  // 同一 tick 连发，模拟「启动→停止→再启动」挤在一起。stop 会认领第一次启动并把它关掉，
  // 所以第三次 start 必须等收尾完再新起一次监听，而不是复用那次正在被拆除的尝试——
  // 复用了就会 resolve 出一个实际没人监听的端口
  const first = proxy.start();
  const stopping = proxy.stop();
  const third = proxy.start();

  const result = await third;
  await Promise.all([first, stopping]);

  assert.equal(result.port, fixedPort);
  assert.equal(proxy.getStatus().running, true, '第三次 start resolve 后应处于运行态');
  assert.equal(proxy.getStatus().port, fixedPort);

  // 正面证据：此刻端口确实被代理占着，别人 listen 不上——光看状态位不够
  const conflict = http.createServer(() => {});
  const conflictError = await new Promise((resolve) => {
    conflict.once('error', resolve);
    conflict.listen(fixedPort, '127.0.0.1', () => resolve(null));
  });
  assert.ok(conflictError, '第三次 start resolve 后端口应真的被监听');
  assert.equal(conflictError.code, 'EADDRINUSE');

  await proxy.stop();
  assert.equal(proxy.getStatus().running, false);

  // 端口真的释放了
  const recheck = http.createServer(() => {});
  await new Promise((resolve, reject) => {
    recheck.once('error', reject);
    recheck.listen(fixedPort, '127.0.0.1', resolve);
  });
  await stopServer(recheck);
});

// 日志 entry 在响应结束后才写，request() resolve 与 onLog 之间没有先后保证，轮询等一小段
function waitUntil(predicate, timeoutMs = 2000) {
  const deadline = Date.now() + timeoutMs;
  return new Promise((resolve, reject) => {
    const tick = () => {
      if (predicate()) return resolve();
      if (Date.now() > deadline) return reject(new Error('等待条件超时'));
      setTimeout(tick, 10);
    };
    tick();
  });
}

function baseConfig(port, mockPort) {
  return {
    settings: { port },
    protocols: [{
      id: 'anthropic', name: 'Anthropic', enabled: true,
      providers: [{ id: 'p1', name: 'mock上游', enabled: true, baseUrl: `http://127.0.0.1:${mockPort}`, token: 'sk-real-token', models: ['glm-4.6'] }],
      mappings: [
        { id: 'm1', enabled: true, clientModel: 'cnsinda-glm-4.6', providerId: 'p1', realModel: 'glm-4.6' },
        { id: 'm9', enabled: true, clientModel: '*', providerId: 'p1', realModel: 'glm-4.6' }
      ]
    }]
  };
}

test('端到端：流式 usage 落进日志 entry，message_delta 的累计值不翻倍', async () => {
  const mock = await startMockServer((req, res) => {
    const chunks = [];
    req.on('data', (c) => chunks.push(c));
    req.on('end', () => {
      res.writeHead(200, { 'content-type': 'text/event-stream' });
      res.write('event: message_start\ndata: {"type":"message_start","message":{"model":"glm-4.6","usage":{"input_tokens":120,"output_tokens":1}}}\n\n');
      res.write('event: message_delta\ndata: {"type":"message_delta","delta":{},"usage":{"output_tokens":88}}\n\n');
      res.end();
    });
  });

  const entries = [];
  const proxy = createLocalProxy({ getConfig: () => baseConfig(0, mock.port), onLog: (entry) => entries.push(entry) });
  const { port } = await proxy.start();

  const resp = await request(port, 'POST', '/v1/messages', {
    'content-type': 'application/json',
    'x-api-key': 'sk-fake',
    'anthropic-version': '2023-06-01',
    'accept': 'text/event-stream'
  }, claudeBody('cnsinda-glm-4.6', true));

  assert.equal(resp.status, 200);
  await waitUntil(() => entries.length === 1);
  const entry = entries[0];
  assert.equal(entry.outcome, 'forwarded');
  assert.equal(entry.statusCode, 200);
  assert.equal(entry.matchKind, 'exact');
  assert.equal(entry.matchedRule, 'cnsinda-glm-4.6');
  assert.equal(entry.providerId, 'p1');
  assert.equal(entry.realModel, 'glm-4.6');
  assert.deepEqual(entry.tokens, { input: 120, output: 88 });
  assert.ok(entry.id); // 列表 key 用
  assert.ok(entry.detection && entry.detection.scores);

  await proxy.stop();
  await stopServer(mock.server);
});

test('端到端：客户端中途断开记为取消，不污染上游错误率', async () => {
  // 上游开了流但不结束：模拟生成到一半用户按了 Esc
  const mock = await startMockServer((req, res) => {
    const chunks = [];
    req.on('data', (c) => chunks.push(c));
    req.on('end', () => {
      res.writeHead(200, { 'content-type': 'text/event-stream' });
      res.write('event: message_start\ndata: {"type":"message_start"}\n\n');
    });
  });

  const entries = [];
  const proxy = createLocalProxy({ getConfig: () => baseConfig(0, mock.port), onLog: (entry) => entries.push(entry) });
  const { port } = await proxy.start();

  const clientReq = http.request({
    hostname: '127.0.0.1', port, method: 'POST', path: '/v1/messages',
    headers: { 'content-type': 'application/json', 'x-api-key': 'sk-fake', 'anthropic-version': '2023-06-01', accept: 'text/event-stream' }
  }, (res) => {
    res.on('data', () => clientReq.destroy());
  });
  clientReq.on('error', () => {}); // destroy 触发的连接重置是预期行为，吞掉
  clientReq.end(claudeBody('cnsinda-glm-4.6', true));

  await waitUntil(() => entries.length === 1);
  assert.equal(entries[0].outcome, 'forwarded');
  assert.equal(entries[0].cancelled, true);
  assert.equal(entries[0].statusCode, 502);

  await proxy.stop();
  mock.server.closeAllConnections();
  await stopServer(mock.server);
});

test('端到端：上游连不上记为失败（非取消），错误信息带进 entry', async () => {
  const entries = [];
  const config = {
    settings: { port: 0 },
    protocols: [{
      id: 'openai', name: 'OpenAI', enabled: true,
      providers: [{ id: 'q1', name: '不存在', enabled: true, baseUrl: 'http://127.0.0.1:9', token: 't', models: ['x'] }],
      mappings: [{ id: 'm1', enabled: true, clientModel: 'gpt-4o', providerId: 'q1', realModel: 'x' }]
    }]
  };
  const proxy = createLocalProxy({ getConfig: () => config, onLog: (entry) => entries.push(entry) });
  const { port } = await proxy.start();

  const resp = await request(port, 'POST', '/v1/chat/completions', { 'content-type': 'application/json', authorization: 'Bearer x' }, openaiBody('gpt-4o'));
  assert.equal(resp.status, 502);
  await waitUntil(() => entries.length === 1);
  assert.equal(entries[0].cancelled, false);
  assert.equal(entries[0].statusCode, 502);
  assert.ok(entries[0].error);

  await proxy.stop();
});

test('端到端：模型列表接管与识别失败各自留下可判读的 entry', async () => {
  const entries = [];
  const config = {
    settings: { port: 0 },
    protocols: [{
      id: 'anthropic', name: 'Anthropic', enabled: true,
      providers: [{ id: 'p1', name: 'mock', enabled: true, baseUrl: 'http://127.0.0.1:9', token: 't', models: ['glm-4.6'] }],
      mappings: [{ id: 'm1', enabled: true, clientModel: 'claude-*', providerId: 'p1', realModel: 'glm-4.6' }]
    }]
  };
  const proxy = createLocalProxy({ getConfig: () => config, onLog: (entry) => entries.push(entry) });
  const { port } = await proxy.start();

  // 模型列表：代理自己接管，outcome = models
  const models = await request(port, 'GET', '/v1/models', { 'x-api-key': 'sk-fake', 'anthropic-version': '2023-06-01' });
  assert.equal(models.status, 200);

  // 一个既不像 anthropic 也不像 openai 的请求：outcome = detect_failed，带打分明细
  const weird = await request(port, 'POST', '/whatever', { 'content-type': 'text/plain' }, Buffer.from('plain text'));
  assert.equal(weird.status, 400);

  await waitUntil(() => entries.length === 2);
  assert.equal(entries.find((e) => e.outcome === 'models').statusCode, 200);
  const failed = entries.find((e) => e.outcome === 'detect_failed');
  assert.equal(failed.statusCode, 400);
  assert.ok(failed.detection.scores && Array.isArray(failed.detection.reasons));

  await proxy.stop();
});

test('端到端：请求体超上限记 read_failed，日志页能查到这次 413', async () => {
  const entries = [];
  const config = {
    settings: { port: 0 },
    protocols: [{
      id: 'openai', name: 'OpenAI', enabled: true,
      providers: [{ id: 'q1', name: 'mock', enabled: true, baseUrl: 'http://127.0.0.1:9', token: 't', models: ['x'] }],
      mappings: [{ id: 'm1', enabled: true, clientModel: 'gpt-4o', providerId: 'q1', realModel: 'x' }]
    }]
  };
  const proxy = createLocalProxy({ getConfig: () => config, onLog: (entry) => entries.push(entry), maxBodyBytes: 1024 });
  const { port } = await proxy.start();

  const resp = await request(port, 'POST', '/v1/chat/completions', { 'content-type': 'application/json' }, Buffer.alloc(4096));
  assert.equal(resp.status, 413);
  await waitUntil(() => entries.length === 1);
  assert.equal(entries[0].outcome, 'read_failed');
  assert.equal(entries[0].statusCode, 413);
  assert.equal(entries[0].errorCode, 'body_too_large');
  assert.ok(entries[0].error);

  await proxy.stop();
});
