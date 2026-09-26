// JSON 定点改写单测：只换目标值的字节区间，缩进、键序、转义、行尾一个字节都不动
'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const { setObjectValues } = require('../../electron/services/clients/json');

const VALUES = { ANTHROPIC_BASE_URL: 'http://127.0.0.1:18888', ANTHROPIC_AUTH_TOKEN: 't_abc123' };

test('只换 env 里那两个值，嵌套对象与数组里的同名键不受影响', () => {
  const before = [
    '{',
    '  "hooks": { "PreToolUse": [ { "matcher": "Bash", "hooks": [ { "type": "command" } ] } ] },',
    '  "env": {',
    '    "ANTHROPIC_BASE_URL": "http://192.168.60.2:9005",',
    '    "ANTHROPIC_AUTH_TOKEN": "sk-old",',
    '    "NOTE": "env 里写着 { \\"brace\\": 1 } 也不算数"',
    '  }',
    '}'
  ].join('\n');

  const result = setObjectValues(before, 'env', VALUES);
  assert.equal(result.changed, true);
  assert.equal(
    result.text,
    before
      .replace('"http://192.168.60.2:9005"', '"http://127.0.0.1:18888"')
      .replace('"sk-old"', '"t_abc123"')
  );
});

test('值本来就等于目标值就不动它（连一个字节都不碰）', () => {
  const before = '{"env":{"ANTHROPIC_BASE_URL":"http://127.0.0.1:18888","ANTHROPIC_AUTH_TOKEN":"other"}}';
  const result = setObjectValues(before, 'env', VALUES);
  assert.equal(result.text, '{"env":{"ANTHROPIC_BASE_URL":"http://127.0.0.1:18888","ANTHROPIC_AUTH_TOKEN":"t_abc123"}}');

  const done = setObjectValues(result.text, 'env', VALUES);
  assert.equal(done.changed, false);
  assert.equal(done.text, result.text);
});

test('缺的键按文件自己的缩进单位补进去（4 空格的文件补出来还是 4 空格）', () => {
  const before = [
    '{',
    '    "env": {',
    '        "ANTHROPIC_BASE_URL": "http://192.168.60.2:9005"',
    '    }',
    '}'
  ].join('\n');

  const result = setObjectValues(before, 'env', VALUES);
  assert.equal(
    result.text,
    [
      '{',
      '    "env": {',
      '        "ANTHROPIC_BASE_URL": "http://127.0.0.1:18888",',
      '        "ANTHROPIC_AUTH_TOKEN": "t_abc123"',
      '    }',
      '}'
    ].join('\n')
  );
});

test('env 不存在：整块补在最后一个成员后面，缩进跟着文件走', () => {
  const before = ['{', '  "model": "x",', '  "permissions": { "allow": [] }', '}'].join('\n');
  const result = setObjectValues(before, 'env', VALUES);
  assert.equal(
    result.text,
    [
      '{',
      '  "model": "x",',
      '  "permissions": { "allow": [] },',
      '  "env": {',
      '    "ANTHROPIC_BASE_URL": "http://127.0.0.1:18888",',
      '    "ANTHROPIC_AUTH_TOKEN": "t_abc123"',
      '  }',
      '}'
    ].join('\n')
  );
});

test('根对象是空的、或 env 写成了非对象，都能补出一个能用的 env', () => {
  const emptyRoot = setObjectValues('{}\n', 'env', VALUES);
  assert.equal(
    emptyRoot.text,
    ['{', '  "env": {', '    "ANTHROPIC_BASE_URL": "http://127.0.0.1:18888",', '    "ANTHROPIC_AUTH_TOKEN": "t_abc123"', '  }', '}'].join('\n') + '\n'
  );

  // env 是字符串：这一块归我们写，原地换成一个对象
  const broken = setObjectValues('{"env":"oops"}', 'env', VALUES);
  assert.equal(JSON.parse(broken.text).env.ANTHROPIC_AUTH_TOKEN, 't_abc123');
  assert.ok(!broken.text.includes('oops'));
});

test('键名带转义也认得出（JSON 里 "a\\u0062c" 就是 abc）', () => {
  const before = '{"env":{"ANTHROPIC\\u005fBASE_URL":"http://192.168.60.2:9005"}}';
  const result = setObjectValues(before, 'env', VALUES);
  assert.ok(result.text.includes('"http://127.0.0.1:18888"'));
  // 目标键不在（那个转义键就是它）→ token 那条是新插的
  assert.equal(JSON.parse(result.text).env.ANTHROPIC_BASE_URL, 'http://127.0.0.1:18888');
});

test('压成一行的 JSON 改完还是合法的，别的键不动', () => {
  const before = '{"model":"x","env":{"ANTHROPIC_AUTH_TOKEN":"sk-old"},"z":[1,2,3]}';
  const result = setObjectValues(before, 'env', VALUES);
  const parsed = JSON.parse(result.text);
  assert.equal(parsed.model, 'x');
  assert.deepEqual(parsed.z, [1, 2, 3]);
  assert.equal(parsed.env.ANTHROPIC_AUTH_TOKEN, 't_abc123');
  assert.equal(parsed.env.ANTHROPIC_BASE_URL, 'http://127.0.0.1:18888');
});

test('不是合法 JSON / 根不是对象 → 抛错拒绝写入', () => {
  assert.throws(() => setObjectValues('{ 这不是 json', 'env', VALUES), /不是合法 JSON，为免写坏已放弃/);
  assert.throws(() => setObjectValues('[1,2]', 'env', VALUES), /根节点不是对象，为免写坏已放弃/);
});
