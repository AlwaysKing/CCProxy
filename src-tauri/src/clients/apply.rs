// 把 CCProxy 的配置写进客户端配置文件：claude 的 settings.json、codex 的 config.toml
// 与模型清单 models.json。只改「指向哪儿 + 用哪个 token」和归我们管的模型条目这几个
// 字节，其余内容一个字都不碰——用户的模型映射、权限、插件、注释、缩进、行尾原样保留。
// 写前备份一份 <文件名>.ccproxy-bak（覆盖式，只留最近一次），写用 tmp + rename 原子替换。

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use crate::clients::catalog::{catalog_file_path, empty_catalog_text, rewrite_catalog, CATALOG_FILE, CATALOG_KEY, ExpectedEntry};
use crate::clients::check::{inspect_client_config, InspectContext};
use crate::clients::json::set_object_values;
use crate::clients::toml::{scan_toml_lines, LineKind};
use crate::config::ClientDir;

const BACKUP_SUFFIX: &str = ".ccproxy-bak";
const TEMP_SUFFIX: &str = ".ccproxy-tmp";
// codex 那边由 CCProxy 维护的 provider id：认它就知道哪些内容归我们管，别的块一概不碰
pub const CODEX_PROVIDER_ID: &str = "ccproxy";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteResult {
    pub file: String,
    pub backup_path: Option<String>,
    pub changed: bool,
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) {}

#[cfg(unix)]
fn file_mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).map(|meta| meta.permissions().mode() & 0o777).unwrap_or(0o600)
}

#[cfg(not(unix))]
fn file_mode(_path: &Path) -> u32 {
    0o600
}

// 原子写：同目录下的临时文件 + rename，避免写一半掉电留下半个配置文件。
// 权限沿用原文件的——这是用户的配置，不该因为我们写过就换了权限
fn write_file_atomic(target_path: &Path, content: &str) -> Result<(), String> {
    let mode = file_mode(target_path);
    let temp_path = PathBuf::from(format!("{}{TEMP_SUFFIX}", target_path.to_string_lossy()));
    // 上次崩溃可能留下同名的临时文件，它的权限是旧的，先清掉再写
    let _ = fs::remove_file(&temp_path);
    fs::write(&temp_path, content).map_err(|error| error.to_string())?;
    // write 的权限只是「请求值」，新建文件时会被 umask 削一道，显式 chmod 才是真的沿用原文件权限
    set_mode(&temp_path, mode);
    fs::rename(&temp_path, target_path).map_err(|error| error.to_string())
}

// 配置文件本身是软链时，写它指向的真实文件。rename 会把链接本身换成普通文件——
// 用 dotfiles 仓库管配置的人会因此丢掉链接，而他仓库里那份反而不更新。
// 只在确实是软链时才解开：普通文件保持原路径，免得把 /var 这类系统软链也一起解开
fn resolve_target(file_path: &Path) -> PathBuf {
    if fs::symlink_metadata(file_path).map(|meta| meta.file_type().is_symlink()).unwrap_or(false) {
        if let Ok(real) = fs::canonicalize(file_path) {
            return real;
        }
    }
    file_path.to_path_buf()
}

fn backup_file(target_path: &Path) -> Option<String> {
    if !target_path.exists() {
        return None;
    }
    let backup_path = PathBuf::from(format!("{}{BACKUP_SUFFIX}", target_path.to_string_lossy()));
    fs::copy(target_path, &backup_path).ok()?;
    Some(backup_path.to_string_lossy().to_string())
}

// 内容没变化就不落盘：不为一个字节都没改的文件动备份与 mtime。
// changed 标记跟着返回——新建文件没有备份可留，不能拿 backupPath 判「写没写」
fn write_if_changed(file_path: &Path, text: &str, changed: bool) -> Result<WriteResult, String> {
    let file = file_path.to_string_lossy().to_string();
    if !changed {
        return Ok(WriteResult { file, backup_path: None, changed: false });
    }
    let target = resolve_target(file_path);
    let backup_path = backup_file(&target);
    write_file_atomic(&target, text)?;
    Ok(WriteResult { file, backup_path, changed: true })
}

fn apply_claude_config(dir_path: &str, port: u16, token: &str) -> Result<Vec<WriteResult>, String> {
    let file_path = Path::new(dir_path).join("settings.json");
    // 文件还不存在时给一个空对象当底子，下面的插入逻辑会补出该有的东西
    let text = fs::read_to_string(&file_path).unwrap_or_else(|_| "{}\n".to_string());

    // 只换 env 里这两个值的字节区间：缩进、键序、\u 转义、权限、插件、hooks 一个字不动。
    // JSON 读不懂/结构不对会抛错——那份文件里还有用户的东西，重建整份损失太大
    let next = set_object_values(
        &text,
        "env",
        &[
            ("ANTHROPIC_BASE_URL".to_string(), Value::String(format!("http://127.0.0.1:{port}"))),
            ("ANTHROPIC_AUTH_TOKEN".to_string(), Value::String(token.to_string())),
        ],
    )
    .map_err(|error| format!("settings.json {error}"))?;

    Ok(vec![write_if_changed(&file_path, &next.text, next.changed)?])
}

// 我们那个 provider 块的内容。requires_openai_auth 必须显式写 false：
// codex 默认要求 ChatGPT 登录凭据，那样就不走 experimental_bearer_token 了
fn build_codex_provider_entries(port: u16, token: &str) -> Vec<String> {
    vec![
        "name = \"ccproxy\"".to_string(),
        format!("base_url = \"http://127.0.0.1:{port}/v1\""),
        "wire_api = \"responses\"".to_string(),
        "requires_openai_auth = false".to_string(),
        format!("experimental_bearer_token = {}", serde_json::to_string(token).unwrap_or_default()),
    ]
}

// 逐行改写：顶层 model_provider 指向我们，[model_providers.ccproxy] 整块重写成上面的内容，
// catalogPath 有值时顶层 model_catalog_json 一并指向我们的模型清单，其余行（用户自己的
// provider 块、projects、features、多行字符串）连同它们自己的行尾原样搬过去——
// CRLF 的文件改完还是 CRLF，末尾没有换行的也不给加上
pub fn rewrite_codex_config(text: &str, port: u16, token: &str, catalog_path: Option<&str>) -> Result<String, String> {
    let section_name = format!("model_providers.{CODEX_PROVIDER_ID}");
    let scan = scan_toml_lines(text);
    // 三引号没闭合的文件不是合法 TOML，扫描器会把后面整段都当成字符串内容，
    // 照着写就会把配置插进字符串里面，宁可不写
    if scan.unterminated {
        return Err("config.toml 里有多行字符串没闭合，为免写坏已放弃".to_string());
    }

    let entries = build_codex_provider_entries(port, token);
    // 路径里的反斜杠引号 JSON.stringify 转出来的就是 TOML 基本串的写法，直接用
    let catalog_line = match catalog_path {
        Some(path) => format!("{CATALOG_KEY} = {}", serde_json::to_string(path).unwrap_or_default()),
        None => String::new(),
    };
    let mut output: Vec<String> = Vec::new();
    // 我们新插入的行用哪种行尾：跟文件自己的一致，别把 CRLF 的文件写成两种行尾的混合
    let mut file_eol = String::new();
    let mut wrote_model_provider = false;
    let mut wrote_catalog = false;
    let mut wrote_provider = false;
    let mut skipping_provider_body = false;

    for line in &scan.lines {
        if file_eol.is_empty() && !line.eol.is_empty() {
            file_eol = line.eol.clone();
        }
        // 这一行自己没带行尾（文件末尾那种）时，我们在它前后插的行用文件惯用的行尾
        let nl = if line.eol.is_empty() {
            if file_eol.is_empty() { "\n".to_string() } else { file_eol.clone() }
        } else {
            line.eol.clone()
        };

        if line.kind == LineKind::Section {
            skipping_provider_body = false;
            // TOML 的顶层键必须写在第一个节之前，所以插入点就是第一个节名这行
            if !wrote_model_provider {
                output.push(format!("model_provider = \"{CODEX_PROVIDER_ID}\"{nl}"));
                wrote_model_provider = true;
            }
            if !catalog_line.is_empty() && !wrote_catalog {
                output.push(format!("{catalog_line}{nl}"));
                wrote_catalog = true;
            }
            output.push(format!("{}{}", line.text, line.eol));
            if line.section != section_name {
                continue;
            }
            // 这个块由 CCProxy 维护，整块替换；块里的旧键与注释一并去掉，不留新旧两套
            for entry in &entries {
                output.push(format!("{entry}{nl}"));
            }
            wrote_provider = true;
            skipping_provider_body = true;
            continue;
        }

        if skipping_provider_body {
            // 空行留着当分隔，块体的键值与注释都跳过
            if line.text.trim().is_empty() {
                output.push(format!("{}{}", line.text, line.eol));
            }
            continue;
        }

        if line.kind == LineKind::Entry && line.section.is_empty() && line.key == "model_provider" {
            if line.value == CODEX_PROVIDER_ID {
                // 已经指向我们了：整行原样留着（行尾注释、单双引号写法都是用户自己的）
                output.push(format!("{}{}", line.text, line.eol));
            } else if line.value_end > line.value_start {
                // 只把值 token 换掉，行尾注释与缩进不动
                output.push(format!(
                    "{}\"{CODEX_PROVIDER_ID}\"{}{}",
                    &line.text[..line.value_start],
                    &line.text[line.value_end..],
                    line.eol
                ));
            } else {
                output.push(format!("model_provider = \"{CODEX_PROVIDER_ID}\"{nl}"));
            }
            wrote_model_provider = true;
            continue;
        }

        if !catalog_line.is_empty() && line.kind == LineKind::Entry && line.section.is_empty() && line.key == CATALOG_KEY {
            let path = catalog_path.unwrap_or("");
            if line.value == path {
                // 已经指对了：整行原样留着
                output.push(format!("{}{}", line.text, line.eol));
            } else if line.value_end > line.value_start {
                output.push(format!(
                    "{}{}{}{}",
                    &line.text[..line.value_start],
                    serde_json::to_string(path).unwrap_or_default(),
                    &line.text[line.value_end..],
                    line.eol
                ));
            } else {
                output.push(format!("{catalog_line}{nl}"));
            }
            wrote_catalog = true;
            continue;
        }

        output.push(format!("{}{}", line.text, line.eol));
    }

    if !wrote_provider {
        let nl = if file_eol.is_empty() { "\n".to_string() } else { file_eol.clone() };
        let written = output.join("");
        // 追加在末尾：原文最后一行若没有行尾，得先补一个，不然就和我们的块粘在一起了；
        // 再空一行把我们那个块和用户的内容隔开
        if !written.is_empty() && !written.ends_with('\n') && !written.ends_with('\r') {
            output.push(nl.clone());
        }
        output.push(nl.clone());
        output.push(format!("[{section_name}]{nl}"));
        for entry in &entries {
            output.push(format!("{entry}{nl}"));
        }
    }
    // 文件里连一个节都没有时上面插入不了，补在最前面（我们刚追加的块在它后面）。
    // 先补 catalog 再补 model_provider：unshift 是往头上放，后放的排在前面
    if !catalog_line.is_empty() && !wrote_catalog {
        let eol = if file_eol.is_empty() { "\n" } else { &file_eol };
        output.insert(0, format!("{catalog_line}{eol}"));
    }
    if !wrote_model_provider {
        let eol = if file_eol.is_empty() { "\n" } else { &file_eol };
        output.insert(0, format!("model_provider = \"{CODEX_PROVIDER_ID}\"{eol}"));
    }

    Ok(output.join(""))
}

fn apply_codex_config(dir_path: &str, port: u16, token: &str, catalog: &[ExpectedEntry]) -> Result<Vec<WriteResult>, String> {
    let config_path = Path::new(dir_path).join("config.toml");
    let config_text = fs::read_to_string(&config_path).unwrap_or_default();
    // 有归我们管的模型时才管清单：catalog 为空说明 OpenAI 协议下一条映射都没启用，
    // 而 codex 拒绝加载空清单——写一份只有壳的进去反而把人家弄坏
    let catalog_path = if catalog.is_empty() { None } else { Some(catalog_file_path(dir_path)) };

    // 两个文件的改写都先算完（坏 JSON、没闭合的多行字符串在这一步抛），再动磁盘——
    // 免得 config.toml 写好了、models.json 却放弃，留下半套状态
    let next_config = rewrite_codex_config(&config_text, port, token, catalog_path.as_deref())
        .map_err(|error| format!("config.toml {error}"))?;
    let mut catalog_write: Option<(PathBuf, String, bool)> = None;
    if catalog_path.is_some() {
        let file_path = Path::new(dir_path).join(CATALOG_FILE);
        let text = fs::read_to_string(&file_path).unwrap_or_else(|_| empty_catalog_text());
        let next = rewrite_catalog(&text, catalog).map_err(|error| format!("{CATALOG_FILE} {error}"))?;
        catalog_write = Some((file_path, next.text, next.changed));
    }

    let mut results = vec![write_if_changed(&config_path, &next_config, next_config != config_text)?];
    if let Some((file_path, text, changed)) = catalog_write {
        results.push(write_if_changed(&file_path, &text, changed)?);
    }
    Ok(results)
}

// 写入一个目录的客户端配置。context = (port, token, catalog)，返回动到的文件清单
// （含各自的备份路径；内容没变的文件 backupPath 为 null 且不落盘）
pub fn apply_client_config(dir_path: &str, kind: &str, port: u16, token: &str, catalog: &[ExpectedEntry]) -> Result<Vec<WriteResult>, String> {
    // 空 token 写进去会把客户端的认证一起写坏（比不改更糟），宁可不写
    if token.is_empty() {
        return Err("CCProxy 还没有客户端 token，无法写入".to_string());
    }
    match kind {
        "claude" => apply_claude_config(dir_path, port, token),
        "codex" => apply_codex_config(dir_path, port, token, catalog),
        other => Err(format!("未知的目录类型：{other}")),
    }
}

pub struct SyncResult {
    pub applied: Vec<WriteResult>,
    pub errors: Vec<(String, String)>,
}

// 配置一变就把已归我们管的 codex 目录的模型清单刷新到最新，用户不用再手动点「应用」。
// 「归我们管」的判据是地址与 token 都对（不带清单口径检查——清单不一致正是这次要改的）；
// 没应用过的目录不替用户做主，留着等他在设置页手动应用
pub fn sync_codex_catalogs(entries: &[ClientDir], port: u16, token: &str, catalog: &[ExpectedEntry]) -> SyncResult {
    let mut applied = Vec::new();
    let mut errors = Vec::new();
    for entry in entries {
        if entry.kind != "codex" {
            continue;
        }
        let empty_context = InspectContext::new(port, token, Vec::new());
        if inspect_client_config(&entry.path, "codex", &empty_context).status != "ok" {
            continue;
        }
        match apply_client_config(&entry.path, "codex", port, token, catalog) {
            Ok(results) => applied.extend(results.into_iter().filter(|item| item.changed)),
            Err(error) => errors.push((entry.path.clone(), error)),
        }
    }
    SyncResult { applied, errors }
}

