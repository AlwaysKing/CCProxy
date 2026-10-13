// 把 CCProxy 的配置写进客户端配置文件：claude 的 settings.json、codex 的 config.toml
// 与模型清单 models.json。只改「指向哪儿 + 用哪个 token」和归我们管的模型条目这几个
// 字节，其余内容一个字都不碰——用户的模型映射、权限、插件、注释、缩进、行尾原样保留。
// 写前备份一份 <文件名>.ccproxy-bak（覆盖式，只留最近一次），写用 tmp + rename 原子替换。
// 只依赖 fs/os/path，不 require electron，可以脱离主进程单测。

'use strict';

const fs = require('fs');
const path = require('path');
const { scanTomlLines } = require('./toml');
const { setObjectValues } = require('./json');
const { inspectClientConfig } = require('./check');
const { CATALOG_FILE, CATALOG_KEY, catalogFilePath, emptyCatalogText, rewriteCatalog } = require('./catalog');

const BACKUP_SUFFIX = '.ccproxy-bak';
const TEMP_SUFFIX = '.ccproxy-tmp';
// codex 那边由 CCProxy 维护的 provider id：认它就知道哪些内容归我们管，别的块一概不碰
const CODEX_PROVIDER_ID = 'ccproxy';

// 原子写：同目录下的临时文件 + rename，避免写一半掉电留下半个配置文件。
// 权限沿用原文件的——这是用户的配置，不该因为我们写过就换了权限
function writeFileAtomic(targetPath, content) {
  let mode = 0o600;
  try {
    mode = fs.statSync(targetPath).mode & 0o777;
  } catch {
    // 文件还不存在，用 0600
  }
  const tempPath = `${targetPath}${TEMP_SUFFIX}`;
  // 上次崩溃可能留下同名的临时文件，它的权限是旧的，先清掉再写
  fs.rmSync(tempPath, { force: true });
  fs.writeFileSync(tempPath, content, { mode });
  // writeFileSync 的 mode 只是「请求值」，新建文件时会被 umask 削一道
  // （umask 022 下 0666 变 0644），显式 chmod 才是真的沿用原文件权限
  fs.chmodSync(tempPath, mode);
  fs.renameSync(tempPath, targetPath);
}

// 配置文件本身是软链时，写它指向的真实文件。rename 会把链接本身换成普通文件——
// 用 dotfiles 仓库管配置的人会因此丢掉链接，而他仓库里那份反而不更新。
// 只在确实是软链时才解开：普通文件保持原路径，免得把 /var 这类系统软链也一起解开，
// 回头报给用户一个他没见过的路径
function resolveTarget(filePath) {
  try {
    if (fs.lstatSync(filePath).isSymbolicLink()) return fs.realpathSync(filePath);
  } catch {
    // 文件还不存在（或断链），按原路径新建
  }
  return filePath;
}

function backupFile(targetPath) {
  if (!fs.existsSync(targetPath)) return null;
  const backupPath = `${targetPath}${BACKUP_SUFFIX}`;
  fs.copyFileSync(targetPath, backupPath);
  return backupPath;
}

function applyClaudeConfig(dirPath, { port, token }) {
  const filePath = path.join(dirPath, 'settings.json');
  // 文件还不存在时给一个空对象当底子，下面的插入逻辑会补出该有的东西
  const text = fs.existsSync(filePath) ? fs.readFileSync(filePath, 'utf8') : '{}\n';

  // 只换 env 里这两个值的字节区间：缩进、键序、\u 转义、权限、插件、hooks 一个字不动。
  // JSON 读不懂/结构不对会抛错——那份文件里还有用户的东西，重建整份损失太大
  let next;
  try {
    next = setObjectValues(text, 'env', {
      ANTHROPIC_BASE_URL: `http://127.0.0.1:${port}`,
      ANTHROPIC_AUTH_TOKEN: token
    });
  } catch (error) {
    throw new Error(`settings.json ${error.message}`);
  }
  return [writeIfChanged(filePath, next)];
}

// 我们那个 provider 块的内容。requires_openai_auth 必须显式写 false：
// codex 默认要求 ChatGPT 登录凭据，那样就不走 experimental_bearer_token 了
function buildCodexProviderEntries({ port, token }) {
  return [
    'name = "ccproxy"',
    `base_url = "http://127.0.0.1:${port}/v1"`,
    'wire_api = "responses"',
    'requires_openai_auth = false',
    `experimental_bearer_token = ${JSON.stringify(token)}`
  ];
}

// 逐行改写：顶层 model_provider 指向我们，[model_providers.ccproxy] 整块重写成上面的内容，
// catalogPath 有值时顶层 model_catalog_json 一并指向我们的模型清单，其余行（用户自己的
// provider 块、projects、features、多行字符串）连同它们自己的行尾原样搬过去——
// CRLF 的文件改完还是 CRLF，末尾没有换行的也不给加上
function rewriteCodexConfig(text, { port, token, catalogPath }) {
  const sectionName = `model_providers.${CODEX_PROVIDER_ID}`;
  const { lines, unterminated } = scanTomlLines(text);
  // 三引号没闭合的文件不是合法 TOML，扫描器会把后面整段都当成字符串内容，
  // 照着写就会把配置插进字符串里面，宁可不写
  if (unterminated) throw new Error('config.toml 里有多行字符串没闭合，为免写坏已放弃');

  const entries = buildCodexProviderEntries({ port, token });
  // 路径里的反斜杠引号 JSON.stringify 转出来的就是 TOML 基本串的写法，直接用
  const catalogLine = catalogPath ? `${CATALOG_KEY} = ${JSON.stringify(catalogPath)}` : '';
  const output = [];
  // 我们新插入的行用哪种行尾：跟文件自己的一致，别把 CRLF 的文件写成两种行尾的混合
  let fileEol = '';
  let wroteModelProvider = false;
  let wroteCatalog = false;
  let wroteProvider = false;
  let skippingProviderBody = false;

  for (const line of lines) {
    if (!fileEol && line.eol) fileEol = line.eol;
    // 这一行自己没带行尾（文件末尾那种）时，我们在它前后插的行用文件惯用的行尾
    const nl = line.eol || fileEol || '\n';

    if (line.kind === 'section') {
      skippingProviderBody = false;
      // TOML 的顶层键必须写在第一个节之前，所以插入点就是第一个节名这行
      if (!wroteModelProvider) {
        output.push(`model_provider = "${CODEX_PROVIDER_ID}"${nl}`);
        wroteModelProvider = true;
      }
      if (catalogLine && !wroteCatalog) {
        output.push(`${catalogLine}${nl}`);
        wroteCatalog = true;
      }
      output.push(`${line.text}${line.eol}`);
      if (line.section !== sectionName) continue;
      // 这个块由 CCProxy 维护，整块替换；块里的旧键与注释一并去掉，不留新旧两套
      for (const entry of entries) output.push(`${entry}${nl}`);
      wroteProvider = true;
      skippingProviderBody = true;
      continue;
    }

    if (skippingProviderBody) {
      // 空行留着当分隔，块体的键值与注释都跳过
      if (!line.text.trim()) output.push(`${line.text}${line.eol}`);
      continue;
    }

    if (line.kind === 'entry' && !line.section && line.key === 'model_provider') {
      if (line.value === CODEX_PROVIDER_ID) {
        // 已经指向我们了：整行原样留着（行尾注释、单双引号写法都是用户自己的）
        output.push(`${line.text}${line.eol}`);
      } else if (line.valueEnd > line.valueStart) {
        // 只把值 token 换掉，行尾注释与缩进不动
        output.push(`${line.text.slice(0, line.valueStart)}"${CODEX_PROVIDER_ID}"${line.text.slice(line.valueEnd)}${line.eol}`);
      } else {
        output.push(`model_provider = "${CODEX_PROVIDER_ID}"${nl}`);
      }
      wroteModelProvider = true;
      continue;
    }

    if (catalogLine && line.kind === 'entry' && !line.section && line.key === CATALOG_KEY) {
      if (line.value === catalogPath) {
        // 已经指对了：整行原样留着
        output.push(`${line.text}${line.eol}`);
      } else if (line.valueEnd > line.valueStart) {
        output.push(`${line.text.slice(0, line.valueStart)}${JSON.stringify(catalogPath)}${line.text.slice(line.valueEnd)}${line.eol}`);
      } else {
        output.push(`${catalogLine}${nl}`);
      }
      wroteCatalog = true;
      continue;
    }

    output.push(`${line.text}${line.eol}`);
  }

  if (!wroteProvider) {
    const nl = fileEol || '\n';
    const written = output.join('');
    // 追加在末尾：原文最后一行若没有行尾，得先补一个，不然就和我们的块粘在一起了；
    // 再空一行把我们那个块和用户的内容隔开
    if (written && !/[\r\n]$/.test(written)) output.push(nl);
    output.push(nl);
    output.push(`[${sectionName}]${nl}`);
    for (const entry of entries) output.push(`${entry}${nl}`);
  }
  // 文件里连一个节都没有时上面插入不了，补在最前面（我们刚追加的块在它后面）。
  // 先补 catalog 再补 model_provider：unshift 是往头上放，后放的排在前面
  if (catalogLine && !wroteCatalog) output.unshift(`${catalogLine}${fileEol || '\n'}`);
  if (!wroteModelProvider) output.unshift(`model_provider = "${CODEX_PROVIDER_ID}"${fileEol || '\n'}`);

  return output.join('');
}

// 内容没变化就不落盘：不为一个字节都没改的文件动备份与 mtime。
// changed 标记跟着返回——新建文件没有备份可留，不能拿 backupPath 判「写没写」
function writeIfChanged(filePath, { text, changed }) {
  if (!changed) return { file: filePath, backupPath: null, changed: false };
  const target = resolveTarget(filePath);
  const backupPath = backupFile(target);
  writeFileAtomic(target, text);
  return { file: filePath, backupPath, changed: true };
}

function applyCodexConfig(dirPath, { port, token, catalog }) {
  const configPath = path.join(dirPath, 'config.toml');
  const configText = fs.existsSync(configPath) ? fs.readFileSync(configPath, 'utf8') : '';
  // 有归我们管的模型时才管清单：catalog 为空说明 OpenAI 协议下一条映射都没启用，
  // 而 codex 拒绝加载空清单——写一份只有壳的进去反而把人家弄坏
  const catalogPath = catalog && catalog.length ? catalogFilePath(dirPath) : null;

  // 两个文件的改写都先算完（坏 JSON、没闭合的多行字符串在这一步抛），再动磁盘——
  // 免得 config.toml 写好了、models.json 却放弃，留下半套状态
  let nextConfig;
  try {
    nextConfig = rewriteCodexConfig(configText, { port, token, catalogPath });
  } catch (error) {
    throw new Error(`config.toml ${error.message}`);
  }
  let catalogWrite = null;
  if (catalogPath) {
    const filePath = path.join(dirPath, CATALOG_FILE);
    const text = fs.existsSync(filePath) ? fs.readFileSync(filePath, 'utf8') : emptyCatalogText();
    let next;
    try {
      next = rewriteCatalog(text, catalog);
    } catch (error) {
      throw new Error(`${CATALOG_FILE} ${error.message}`);
    }
    catalogWrite = { filePath, next };
  }

  const results = [writeIfChanged(configPath, { text: nextConfig, changed: nextConfig !== configText })];
  if (catalogWrite) results.push(writeIfChanged(catalogWrite.filePath, catalogWrite.next));
  return results;
}

// 写入一个目录的客户端配置。入参 context = { port, token, catalog }，返回动到的文件
// 清单（含各自的备份路径；内容没变的文件 backupPath 为 null 且不落盘）
function applyClientConfig(dirPath, kind, context) {
  // 空 token 写进去会把客户端的认证一起写坏（比不改更糟），宁可不写
  if (!context.token) throw new Error('CCProxy 还没有客户端 token，无法写入');
  if (kind === 'claude') return applyClaudeConfig(dirPath, context);
  if (kind === 'codex') return applyCodexConfig(dirPath, context);
  throw new Error(`未知的目录类型：${kind}`);
}

// 配置一变就把已归我们管的 codex 目录的模型清单刷新到最新，用户不用再手动点「应用」。
// 「归我们管」的判据是地址与 token 都对（不带清单口径检查——清单不一致正是这次要改的）；
// 没应用过的目录不替用户做主，留着等他在设置页手动应用。
// 返回 { applied, errors }：applied 是真正落了盘的文件，errors 按目录收集失败原因
function syncCodexCatalogs(entries, context) {
  const applied = [];
  const errors = [];
  for (const entry of entries) {
    if (!entry || entry.kind !== 'codex') continue;
    if (inspectClientConfig(entry.path, 'codex', { ...context, catalog: [] }).status !== 'ok') continue;
    try {
      const results = applyClientConfig(entry.path, 'codex', context);
      applied.push(...results.filter((item) => item.changed));
    } catch (error) {
      errors.push({ path: entry.path, message: error.message });
    }
  }
  return { applied, errors };
}

module.exports = { applyClientConfig, rewriteCodexConfig, syncCodexCatalogs, CODEX_PROVIDER_ID };
