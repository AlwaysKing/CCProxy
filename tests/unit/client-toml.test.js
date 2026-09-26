// TOML 行扫描单测：行尾注释、引号内的 #、两种三引号多行字符串、带点的节名
'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const { parseTomlValue, scanTomlLines } = require('../../electron/services/clients/toml');

function scan(text) {
  return scanTomlLines(text).lines;
}

function entries(text) {
  return scan(text).filter((line) => line.kind === 'entry');
}

test('取值：引号包住的去引号，引号里的 # 不当注释', () => {
  assert.equal(parseTomlValue('"http://127.0.0.1:18888/v1"'), 'http://127.0.0.1:18888/v1');
  assert.equal(parseTomlValue("'single'"), 'single');
  assert.equal(parseTomlValue('"#0169cc"'), '#0169cc');
  assert.equal(parseTomlValue('true'), 'true');
  assert.equal(parseTomlValue('  42  '), '42');
  // 转义只在双引号里算转义，\' 里的引号不该提前收尾
  assert.equal(parseTomlValue('"a\\"b"'), 'a\\"b');
  assert.equal(parseTomlValue('"a\\"b" # 尾注释'), 'a\\"b');
});

test('取值：行尾注释剥掉（TOML 允许行尾注释，不剥会把注释当成值）', () => {
  assert.equal(parseTomlValue('http://127.0.0.1:18888 # 本地代理'), 'http://127.0.0.1:18888');
  assert.equal(parseTomlValue('"http://127.0.0.1:18888" # 本地代理'), 'http://127.0.0.1:18888');
  assert.equal(parseTomlValue('# 整行注释'), '');
});

test('取值：引号没闭合时退化成裸值，不吞掉后半段', () => {
  assert.equal(parseTomlValue('"unclosed'), 'unclosed');
});

test('扫描：节名、顶层键、节内键分得清', () => {
  const lines = scan([
    'model_provider = "custom"',
    'model = "gpt-5.6-sol"',
    '',
    '# 注释',
    '[model_providers.custom]',
    'base_url = "http://127.0.0.1:18888/v1"'
  ].join('\n'));

  assert.deepEqual(
    lines.map((line) => line.kind),
    ['entry', 'entry', 'text', 'text', 'section', 'entry']
  );
  assert.equal(lines[0].section, '');
  assert.equal(lines[0].key, 'model_provider');
  assert.equal(lines[0].value, 'custom');
  assert.equal(lines[4].section, 'model_providers.custom');
  assert.equal(lines[5].section, 'model_providers.custom');
  assert.equal(lines[5].value, 'http://127.0.0.1:18888/v1');
});

test('扫描：节名后面跟行尾注释照样算节名行（认不出来它就不是块的边界）', () => {
  const lines = scan([
    '[model_providers.custom] # 我自己的备用供应商',
    'base_url = "https://api.example.com/v1"'
  ].join('\n'));

  assert.equal(lines[0].kind, 'section');
  assert.equal(lines[0].section, 'model_providers.custom');
  // 节名行之后的键值要归到它名下，不能因为认不出节名就当成顶层键
  assert.equal(lines[1].kind, 'entry');
  assert.equal(lines[1].section, 'model_providers.custom');
});

test('扫描：节名归一化——引号段去引号，[a."b"] 与 [a.b] 是同一个表', () => {
  assert.equal(scan('[model_providers."ccproxy"]\n')[0].section, 'model_providers.ccproxy');
  assert.equal(scan("[model_providers.'ccproxy']\n")[0].section, 'model_providers.ccproxy');
  assert.equal(scan('[a.b]\n')[0].section, 'a.b');
  // 引号里的 ] 属于名字本身，不能当成节名的结尾
  assert.equal(scan('[model_providers."a]b"]\nx = 1\n')[0].section, 'model_providers.a]b');
});

test('扫描：三引号没闭合时 unterminated 为 true', () => {
  const broken = scanTomlLines('model_provider = "custom"\ndev = """\nmodel_provider = "hijacked"\n');
  assert.equal(broken.unterminated, true);
  // 未闭合时后面那些行只能当字符串内容，不能当键值
  assert.ok(!broken.lines.some((line) => line.value === 'hijacked'));

  assert.equal(scanTomlLines('dev = """\n内容\n"""\nmodel_provider = "custom"\n').unterminated, false);
  assert.equal(scanTomlLines('dev = """一行闭合"""\n').unterminated, false);
});

test('扫描：双引号三引号块内的内容不算键值，model_provider 不会被劫持', () => {
  const lines = scan([
    'model_provider = "custom"',
    'developer_instructions = """',
    'model_provider = "hijacked"',
    '[model_providers.hijack]',
    '"""',
    '[model_providers.custom]',
    'base_url = "http://127.0.0.1:18888/v1"'
  ].join('\n'));

  // 块内那两行必须是 text：既不能变成键值、也不能变成节名
  assert.deepEqual(lines.slice(2, 4).map((line) => line.kind), ['text', 'text']);
  assert.equal(lines.find((line) => !line.section && line.key === 'model_provider').value, 'custom');
  assert.ok(!lines.some((line) => line.value === 'hijacked'));
  assert.ok(!lines.some((line) => line.section === 'model_providers.hijack'));
});

test('扫描：单引号三引号块同样隔离（只认 """ 的话这里会漏）', () => {
  const lines = scan([
    "instructions = '''",
    'model_provider = "hijacked"',
    "'''",
    '[model_providers.custom]',
    'base_url = "http://127.0.0.1:18888/v1"'
  ].join('\n'));

  assert.deepEqual(lines.slice(1, 2).map((line) => line.kind), ['text']);
  assert.ok(!lines.some((line) => line.value === 'hijacked'));
});

test('扫描：同行闭合的三引号不进多行态', () => {
  const lines = scan([
    'note = """一行写完"""',
    'model_provider = "custom"'
  ].join('\n'));
  assert.deepEqual(lines.map((line) => line.kind), ['entry', 'entry']);
  assert.equal(lines[1].key, 'model_provider');
});

test('扫描：缩进与 CRLF 都能处理', () => {
  const lines = scan('  [model_providers.custom]\r\n\tbase_url = "x"  \r\n');
  assert.equal(lines[0].kind, 'section');
  assert.equal(lines[0].section, 'model_providers.custom');
  assert.equal(lines[1].kind, 'entry');
  assert.equal(lines[1].value, 'x');
});

test('扫描：不认识的行算 text，不猜', () => {
  assert.deepEqual(scan(['not a toml line', '= 5', '[]', '[[array]]', '[a] b'].join('\n')).map((line) => line.kind), [
    'text',
    'text',
    'text',
    'text',
    'text'
  ]);
  assert.equal(entries('key = value').length, 1);
});
