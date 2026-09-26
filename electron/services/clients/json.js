// 只改 JSON 文本里目标字段的值，其余内容（缩进、键序、\u 转义、行尾、权限、插件、
// hooks）一个字节都不碰。
// 不用「JSON.parse 再 stringify」是因为那会把整个文件重排版——claude 的 settings.json
// 里还有用户自己的权限、插件、hooks，改两个 env 值不该让全文跟着重排。
// 合法性由 JSON.parse 保证：进来先 parse 一次，拼完再 parse 一次校验。
// 万一下标算歪了，宁可抛错让调用方放弃写入，也不交出一份坏配置。

'use strict';

// 逐个 JSON 值的下标扫描 =====================================================

function skipWhitespace(text, index) {
  let cursor = index;
  while (cursor < text.length && ' \t\r\n'.includes(text[cursor])) cursor += 1;
  return cursor;
}

// 从开引号扫到闭合引号之后；没闭合返回 -1
function scanStringLiteral(text, index) {
  let cursor = index + 1;
  while (cursor < text.length) {
    if (text[cursor] === '\\') {
      cursor += 2;
      continue;
    }
    if (text[cursor] === '"') return cursor + 1;
    cursor += 1;
  }
  return -1;
}

// 扫过任意一个值（含嵌套的对象/数组），返回它之后的下标；结构不对返回 -1。
// 按深度配平括号，字符串整段跳过——字符串里的括号不算数
function scanValue(text, index) {
  const char = text[index];
  if (char === '"') return scanStringLiteral(text, index);
  if (char === '{' || char === '[') {
    let depth = 0;
    let cursor = index;
    while (cursor < text.length) {
      const current = text[cursor];
      if (current === '"') {
        cursor = scanStringLiteral(text, cursor);
        if (cursor === -1) return -1;
        continue;
      }
      if (current === '{' || current === '[') depth += 1;
      else if (current === '}' || current === ']') {
        depth -= 1;
        if (depth === 0) return cursor + 1;
      }
      cursor += 1;
    }
    return -1;
  }
  // 数字 / true / false / null：扫到分隔符为止
  let cursor = index;
  while (cursor < text.length && !',}] \t\r\n'.includes(text[cursor])) cursor += 1;
  return cursor;
}

// 读一个对象，给出它的每个直接成员与闭合花括号的位置；读不成给出 null。
// members 里的 keyStart/keyEnd/valueStart/valueEnd 都是 text 上的下标
function readJsonObject(text, objectStart) {
  if (text[objectStart] !== '{') return null;
  const members = [];
  let cursor = skipWhitespace(text, objectStart + 1);
  if (text[cursor] === '}') return { members, closeIndex: cursor };

  while (cursor < text.length) {
    const keyStart = cursor;
    if (text[keyStart] !== '"') return null;
    const keyEnd = scanStringLiteral(text, keyStart);
    if (keyEnd === -1) return null;
    let name;
    try {
      name = JSON.parse(text.slice(keyStart, keyEnd));
    } catch {
      return null; // 下标算歪了，交给调用方放弃
    }
    cursor = skipWhitespace(text, keyEnd);
    if (text[cursor] !== ':') return null;
    const valueStart = skipWhitespace(text, cursor + 1);
    const valueEnd = scanValue(text, valueStart);
    if (valueEnd === -1) return null;
    members.push({ name, keyStart, keyEnd, valueStart, valueEnd });
    cursor = skipWhitespace(text, valueEnd);
    if (text[cursor] === '}') return { members, closeIndex: cursor };
    if (text[cursor] !== ',') return null;
    cursor = skipWhitespace(text, cursor + 1);
  }
  return null;
}

// 修表 ======================================================================

// index 所在行的缩进（新插入的键照它对齐；已有内容一概不动）
function indentAt(text, index) {
  const lineStart = text.lastIndexOf('\n', Math.max(index - 1, 0)) + 1;
  return text.slice(lineStart).match(/^[ \t]*/)[0];
}

// 文件惯用的缩进单位：所有行里最短的那一段正缩进。缩进是 4 空格的文件就还给 4 空格，
// 全压成一行的文件认不出来，退回两空格
function detectIndentUnit(text) {
  let unit = '';
  for (const line of text.split('\n')) {
    const match = line.match(/^[ \t]+/);
    if (match && (!unit || match[0].length < unit.length)) unit = match[0];
  }
  return unit || '  ';
}

// 从后往前拼，前面的下标才不会被已经改过的内容顶偏
function applyEdits(text, edits) {
  let result = text;
  for (const edit of [...edits].sort((a, b) => b.start - a.start)) {
    result = result.slice(0, edit.start) + edit.text + result.slice(edit.end);
  }
  return result;
}

// 整块新建一个对象（目标字段缺失或不是对象时用）
function buildObjectText(values, indent, unit) {
  const lines = Object.keys(values).map((key) => `${indent}${unit}${JSON.stringify(key)}: ${JSON.stringify(values[key])}`);
  return `{\n${lines.join(',\n')}\n${indent}}`;
}

// 报错都带上「为免写坏已放弃」：调用方只写文件名，用户看到的是一句完整的话
function parseOrThrow(jsonText) {
  try {
    return JSON.parse(jsonText);
  } catch (error) {
    throw new Error(`不是合法 JSON，为免写坏已放弃：${error.message}`);
  }
}

// 把 jsonText 里 objectName（空串表示根对象）那个对象下的若干键设成新值。
// 返回 { text, changed }：只替换目标值的字节区间，值本来就等于目标值就不动它。
// 结构不是预期的样子就抛错，由调用方决定放弃写入
function setObjectValues(jsonText, objectName, values) {
  const text = String(jsonText);
  const parsed = parseOrThrow(text);
  if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
    throw new Error('根节点不是对象，为免写坏已放弃');
  }

  const rootStart = skipWhitespace(text, 0);
  const root = readJsonObject(text, rootStart);
  if (!root) throw new Error('结构看不懂，为免写坏已放弃');
  const unit = detectIndentUnit(text);
  const edits = [];

  let container = root;
  let containerStart = rootStart;

  if (objectName) {
    const member = root.members.find((item) => item.name === objectName);
    const existing = member ? readJsonObject(text, member.valueStart) : null;
    if (existing) {
      container = existing;
      containerStart = member.valueStart;
    } else {
      // 整个对象要么没有、要么不是对象：这一块本来就归我们写，一次换掉
      const indent = root.members.length ? indentAt(text, root.members[0].keyStart) : indentAt(text, root.closeIndex) + unit;
      const block = buildObjectText(values, indent, unit);
      if (member) edits.push({ start: member.valueStart, end: member.valueEnd, text: block });
      else if (root.members.length) {
        const anchor = root.members[root.members.length - 1].valueEnd;
        edits.push({ start: anchor, end: anchor, text: `,\n${indent}${JSON.stringify(objectName)}: ${block}` });
      } else {
        edits.push({ start: rootStart + 1, end: rootStart + 1, text: `\n${indent}${JSON.stringify(objectName)}: ${block}\n${indentAt(text, root.closeIndex)}` });
      }
      return finish(text, edits);
    }
  }

  const keyIndent = container.members.length
    ? indentAt(text, container.members[0].keyStart)
    : indentAt(text, container.closeIndex) + unit;

  // 缺的键一次插完：分两次插会在同一个位置上打架
  const missing = Object.keys(values).filter((key) => !container.members.some((item) => item.name === key));
  if (missing.length) {
    const block = missing.map((key) => `${keyIndent}${JSON.stringify(key)}: ${JSON.stringify(values[key])}`).join(',\n');
    if (container.members.length) {
      const anchor = container.members[container.members.length - 1].valueEnd;
      edits.push({ start: anchor, end: anchor, text: `,\n${block}` });
    } else {
      const closeIndent = indentAt(text, container.closeIndex);
      edits.push({ start: containerStart + 1, end: containerStart + 1, text: `\n${block}\n${closeIndent}` });
    }
  }

  for (const [key, value] of Object.entries(values)) {
    const member = container.members.find((item) => item.name === key);
    if (!member) continue;
    // 值本来就是目标值就别动它，连一个字节都不碰（转义写法不同但值相同也算不碰）
    let current;
    try {
      current = JSON.parse(text.slice(member.valueStart, member.valueEnd));
    } catch {
      current = undefined;
    }
    if (current === value) continue;
    edits.push({ start: member.valueStart, end: member.valueEnd, text: JSON.stringify(value) });
  }

  return finish(text, edits);
}

// 拼回去再 parse 一遍：下标算错了宁可抛错，也不要交出坏配置
function finish(text, edits) {
  if (!edits.length) return { text, changed: false };
  const result = applyEdits(text, edits);
  try {
    JSON.parse(result);
  } catch (error) {
    throw new Error(`按目标字段改写后不是合法 JSON，为免写坏已放弃：${error.message}`);
  }
  return { text: result, changed: true };
}

module.exports = { setObjectValues };
