// TOML 行扫描：把原文按行分成「节名 / 键值 / 其它」，供检查与改写共用。
// 不实现完整 TOML（项目里也没有 TOML 依赖）——codex 那边我们只关心顶层 model_provider
// 和 [model_providers.<id>] 块里的几个键。

'use strict';

// 取一个值的字面内容。引号包住的取到匹配的结束引号为止（引号里的 # 不是注释，
// 例如 accent = "#0169cc"）；裸值取到行尾注释之前——TOML 允许行尾注释，
// 不剥掉就会把 "http://127.0.0.1:18888" # 本地代理 整个当成值，判定全错
function parseTomlValue(raw) {
  const text = String(raw).trim();
  const quote = text[0];
  if (quote === '"' || quote === "'") {
    let index = 1;
    while (index < text.length) {
      // 转义只在双引号字符串里有意义
      if (quote === '"' && text[index] === '\\') {
        index += 2;
        continue;
      }
      if (text[index] === quote) return text.slice(1, index);
      index += 1;
    }
    // 引号没闭合：当裸值处理，让调用方看到原文，比猜它省略了什么强
    return text.slice(1).trim();
  }
  const comment = text.indexOf('#');
  return (comment === -1 ? text : text.slice(0, comment)).trim();
}

// 值 token 占多少个字符：引号包住的算到闭合引号，裸值算到行尾注释之前。
// 改写时靠它只把值换掉，行尾注释与缩进留给调用方原样保留
function valueTokenLength(raw) {
  const text = String(raw).trim();
  const quote = text[0];
  if (quote === '"' || quote === "'") {
    let index = 1;
    while (index < text.length) {
      if (quote === '"' && text[index] === '\\') {
        index += 2;
        continue;
      }
      if (text[index] === quote) return index + 1;
      index += 1;
    }
    return text.length;
  }
  const comment = text.indexOf('#');
  return comment === -1 ? text.length : comment;
}

// 多行字符串的分隔符。""" 与 ''' 都要认：只认一种的话，另一种里面的内容
// 会被当成顶层键读进来（里面的 model_provider 能把真正的那个顶掉）
const MULTILINE_DELIMITERS = ['"""', "'''"];
// 键可以是裸的、双引号包住的或单引号包住的（[model_providers."a.b"] 那种带点的名字）
const ENTRY_PATTERN = /^([A-Za-z0-9_-]+|"[^"]*"|'[^']*')\s*=\s*(.*)$/;

// 节的表名归一化：TOML 里 [a.b] 与 [a."b"] 是同一个表，带引号的段要去掉引号再拼。
// 不归一的话 [model_providers."ccproxy"] 会被当成另一个表，写入时会追加出第二个同名块
function normalizeSectionPath(name) {
  const segments = [];
  let buffer = '';
  let quote = '';
  for (const char of name) {
    if (quote) {
      if (char === quote) quote = '';
      else buffer += char;
      continue;
    }
    if (char === '"' || char === "'") {
      quote = char;
      continue;
    }
    if (char === '.') {
      segments.push(buffer.trim());
      buffer = '';
      continue;
    }
    buffer += char;
  }
  segments.push(buffer.trim());
  return segments.join('.');
}

// 认一行是不是节名行，是就给出归一化后的表名，不是给空串。
// 不能只按「整行以 ] 结尾」来认：TOML 允许节名后面跟行尾注释，
// [model_providers.x] # 说明 认不出来的话它就成不了「块的边界」，
// 改写时后面那一整段会连内容带节名一起被当成块内内容删掉
function parseSectionName(trimmed) {
  if (!trimmed.startsWith('[')) return '';
  // 数组表 [[x]] 我们不管，当普通文本，免得跟节名混起来
  if (trimmed.startsWith('[[')) return '';
  let index = 1;
  let quote = '';
  while (index < trimmed.length) {
    const char = trimmed[index];
    if (quote) {
      if (char === quote) quote = '';
    } else if (char === '"' || char === "'") {
      quote = char;
    } else if (char === ']') {
      break;
    }
    index += 1;
  }
  if (index >= trimmed.length) return ''; // 没有闭合的 ]
  const rest = trimmed.slice(index + 1).trim();
  if (rest && !rest.startsWith('#')) return ''; // ] 后面只允许行尾注释
  return normalizeSectionPath(trimmed.slice(1, index));
}

// 按行切分，并保留每行自己的行尾。改写要「除了改的那几个字节，其余一个字节都不碰」，
// 顺手把 CRLF 的文件整成 LF 就是碰了，所以内容与行尾必须分开拿着
function splitLines(text) {
  const raw = String(text);
  const lines = [];
  for (const match of raw.matchAll(/([^\r\n]*)(\r\n|\n|\r)?/g)) {
    if (match.index === raw.length) break; // 末尾补出来的空匹配不要
    lines.push({ text: match[1], eol: match[2] || '' });
  }
  return lines;
}

// 每行给出 { text, eol, kind, section, key, value, valueStart, valueEnd }：
// kind = 'section'（节名）/ 'entry'（键值）/ 'text'（空行、注释、多行字符串内部）
// entry 的 section 是它所属节的表名（顶层键为空串），valueStart/valueEnd 是值 token
// 在这行 text 里的区间——只改值、留注释就靠它
// 返回 { lines, unterminated }。三引号没闭合时 unterminated 为 true：这种文件已经不是
// 合法 TOML 了，调用方要据此拒绝解读——尤其别无脑拿它去改写（会写进字符串里面）
function scanTomlLines(text) {
  const lines = [];
  let section = '';
  let multiline = '';

  for (const raw of splitLines(text)) {
    if (multiline) {
      lines.push({ text: raw.text, eol: raw.eol, kind: 'text', section, key: '', value: '', valueStart: 0, valueEnd: 0 });
      // 结束行本身也是字符串内容，收尾后回到正常扫描
      if (raw.text.includes(multiline)) multiline = '';
      continue;
    }

    const trimmed = raw.text.trim();
    if (!trimmed || trimmed.startsWith('#')) {
      lines.push({ text: raw.text, eol: raw.eol, kind: 'text', section, key: '', value: '', valueStart: 0, valueEnd: 0 });
      continue;
    }

    const sectionName = parseSectionName(trimmed);
    if (sectionName) {
      section = sectionName;
      lines.push({ text: raw.text, eol: raw.eol, kind: 'section', section, key: '', value: '', valueStart: 0, valueEnd: 0 });
      continue;
    }

    const entryMatch = trimmed.match(ENTRY_PATTERN);
    if (!entryMatch) {
      lines.push({ text: raw.text, eol: raw.eol, kind: 'text', section, key: '', value: '', valueStart: 0, valueEnd: 0 });
      continue;
    }

    // 值 token 的位置在原始行里算：跳过键、等号与等号后的空白
    const offset = raw.text.indexOf(trimmed);
    const equals = trimmed.indexOf('=', entryMatch[1].length);
    const afterEquals = trimmed.slice(equals + 1);
    const valueText = afterEquals.trim();
    const valueStart = offset + equals + 1 + (afterEquals.length - afterEquals.trimStart().length);
    lines.push({
      text: raw.text,
      eol: raw.eol,
      kind: 'entry',
      section,
      key: parseTomlValue(entryMatch[1]),
      value: parseTomlValue(entryMatch[2]),
      valueStart,
      valueEnd: valueStart + valueTokenLength(valueText)
    });

    const valueRaw = entryMatch[2].trim();
    for (const delimiter of MULTILINE_DELIMITERS) {
      if (valueRaw.startsWith(delimiter) && !valueRaw.slice(delimiter.length).includes(delimiter)) {
        multiline = delimiter;
      }
    }
  }

  return { lines, unterminated: !!multiline };
}

module.exports = { parseTomlValue, scanTomlLines };
