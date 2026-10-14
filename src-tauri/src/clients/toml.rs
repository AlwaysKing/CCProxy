// TOML 行扫描：把原文按行分成「节名 / 键值 / 其它」，供检查与改写共用。
// 不实现完整 TOML——codex 那边我们只关心顶层 model_provider 和
// [model_providers.<id>] 块里的几个键。
//
// 下标一律按字节算：所有分隔符（引号、#、=、[]）都是 ASCII，UTF-8 的多字节序列里
// 不会出现这些字节，所以按字节扫描与按字符扫描等价，切片也永远落在字符边界上。

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineKind {
    Section,
    Entry,
    Text,
}

#[derive(Debug, Clone)]
pub struct TomlLine {
    pub text: String,
    pub eol: String,
    pub kind: LineKind,
    pub section: String,
    pub key: String,
    pub value: String,
    pub value_start: usize,
    pub value_end: usize,
}

pub struct TomlScan {
    pub lines: Vec<TomlLine>,
    pub unterminated: bool,
}

fn trim_ascii_ws(text: &str) -> &str {
    text.trim()
}

// 取一个值的字面内容。引号包住的取到匹配的结束引号为止（引号里的 # 不是注释）；
// 裸值取到行尾注释之前——TOML 允许行尾注释，不剥掉就会把整个「值 + 注释」当成值
pub fn parse_toml_value(raw: &str) -> String {
    let text = trim_ascii_ws(raw);
    let bytes = text.as_bytes();
    if let Some(&quote) = bytes.first() {
        if quote == b'"' || quote == b'\'' {
            let mut index = 1;
            while index < bytes.len() {
                // 转义只在双引号字符串里有意义
                if quote == b'"' && bytes[index] == b'\\' {
                    index += 2;
                    continue;
                }
                if bytes[index] == quote {
                    return text[1..index].to_string();
                }
                index += 1;
            }
            // 引号没闭合：当裸值处理，让调用方看到原文，比猜它省略了什么强
            return text[1..].trim().to_string();
        }
    }
    match text.find('#') {
        Some(index) => text[..index].trim().to_string(),
        None => text.to_string(),
    }
}

// 值 token 占多少个字节：引号包住的算到闭合引号，裸值算到行尾注释之前。
// 改写时靠它只把值换掉，行尾注释与缩进留给调用方原样保留
pub fn value_token_length(raw: &str) -> usize {
    let text = trim_ascii_ws(raw);
    let bytes = text.as_bytes();
    if let Some(&quote) = bytes.first() {
        if quote == b'"' || quote == b'\'' {
            let mut index = 1;
            while index < bytes.len() {
                if quote == b'"' && bytes[index] == b'\\' {
                    index += 2;
                    continue;
                }
                if bytes[index] == quote {
                    return index + 1;
                }
                index += 1;
            }
            return text.len();
        }
    }
    match text.find('#') {
        Some(index) => index,
        None => text.len(),
    }
}

// 节的表名归一化：TOML 里 [a.b] 与 [a."b"] 是同一个表，带引号的段要去掉引号再拼。
// 不归一的话 [model_providers."ccproxy"] 会被当成另一个表，写入时会追加出第二个同名块
pub fn normalize_section_path(name: &str) -> String {
    let mut segments: Vec<String> = Vec::new();
    let mut buffer = String::new();
    let mut quote: Option<char> = None;
    for character in name.chars() {
        match quote {
            Some(active) => {
                if character == active {
                    quote = None;
                } else {
                    buffer.push(character);
                }
            }
            None => {
                if character == '"' || character == '\'' {
                    quote = Some(character);
                } else if character == '.' {
                    segments.push(buffer.trim().to_string());
                    buffer.clear();
                } else {
                    buffer.push(character);
                }
            }
        }
    }
    segments.push(buffer.trim().to_string());
    segments.join(".")
}

// 认一行是不是节名行，是就给出归一化后的表名，不是给空串。
// 不能只按「整行以 ] 结尾」来认：TOML 允许节名后面跟行尾注释
fn parse_section_name(trimmed: &str) -> String {
    if !trimmed.starts_with('[') {
        return String::new();
    }
    // 数组表 [[x]] 我们不管，当普通文本，免得跟节名混起来
    if trimmed.starts_with("[[") {
        return String::new();
    }
    let bytes = trimmed.as_bytes();
    let mut index = 1;
    let mut quote: u8 = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if quote != 0 {
            if byte == quote {
                quote = 0;
            }
        } else if byte == b'"' || byte == b'\'' {
            quote = byte;
        } else if byte == b']' {
            break;
        }
        index += 1;
    }
    if index >= bytes.len() {
        return String::new(); // 没有闭合的 ]
    }
    let rest = trimmed[index + 1..].trim();
    if !rest.is_empty() && !rest.starts_with('#') {
        return String::new(); // ] 后面只允许行尾注释
    }
    normalize_section_path(&trimmed[1..index])
}

// 键值行：键可以是裸的、双引号包住的或单引号包住的（[model_providers."a.b"] 那种带点的名字）
fn match_entry(trimmed: &str) -> Option<(&str, &str)> {
    let bytes = trimmed.as_bytes();
    let mut index = 0;
    match bytes.first()? {
        b'"' | b'\'' => {
            let quote = bytes[0];
            index = 1;
            while index < bytes.len() && bytes[index] != quote {
                index += 1;
            }
            if index >= bytes.len() {
                return None;
            }
            index += 1;
        }
        _ => {
            while index < bytes.len()
                && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_' || bytes[index] == b'-')
            {
                index += 1;
            }
            if index == 0 {
                return None;
            }
        }
    }
    let key = &trimmed[..index];
    while index < bytes.len() && bytes[index].is_ascii_whitespace() {
        index += 1;
    }
    if index >= bytes.len() || bytes[index] != b'=' {
        return None;
    }
    index += 1;
    while index < bytes.len() && bytes[index].is_ascii_whitespace() {
        index += 1;
    }
    Some((key, &trimmed[index..]))
}

// 按行切分，并保留每行自己的行尾。改写要「除了改的那几个字节，其余一个字节都不碰」，
// 顺手把 CRLF 的文件整成 LF 就是碰了，所以内容与行尾必须分开拿着
pub fn split_lines(text: &str) -> Vec<(&str, &str)> {
    let bytes = text.as_bytes();
    let mut lines = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let start = index;
        while index < bytes.len() && bytes[index] != b'\n' && bytes[index] != b'\r' {
            index += 1;
        }
        let text_end = index;
        let eol_start = index;
        if index < bytes.len() {
            if bytes[index] == b'\r' && index + 1 < bytes.len() && bytes[index + 1] == b'\n' {
                index += 2;
            } else {
                index += 1;
            }
        }
        lines.push((&text[start..text_end], &text[eol_start..index]));
    }
    lines
}

// 多行字符串的分隔符。""" 与 ''' 都要认：只认一种的话，另一种里面的内容
// 会被当成顶层键读进来（里面的 model_provider 能把真正的那个顶掉）
const MULTILINE_DELIMITERS: [&str; 2] = ["\"\"\"", "'''"];

// 每行给出 { text, eol, kind, section, key, value, value_start, value_end }。
// 三引号没闭合时 unterminated 为 true：这种文件已经不是合法 TOML 了，
// 调用方要据此拒绝解读——尤其别无脑拿它去改写（会写进字符串里面）
pub fn scan_toml_lines(text: &str) -> TomlScan {
    let mut lines: Vec<TomlLine> = Vec::new();
    let mut section = String::new();
    let mut multiline = String::new();

    for (raw_text, raw_eol) in split_lines(text) {
        if !multiline.is_empty() {
            lines.push(TomlLine {
                text: raw_text.to_string(),
                eol: raw_eol.to_string(),
                kind: LineKind::Text,
                section: section.clone(),
                key: String::new(),
                value: String::new(),
                value_start: 0,
                value_end: 0,
            });
            // 结束行本身也是字符串内容，收尾后回到正常扫描
            if raw_text.contains(&multiline) {
                multiline.clear();
            }
            continue;
        }

        let trimmed = raw_text.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            lines.push(TomlLine {
                text: raw_text.to_string(),
                eol: raw_eol.to_string(),
                kind: LineKind::Text,
                section: section.clone(),
                key: String::new(),
                value: String::new(),
                value_start: 0,
                value_end: 0,
            });
            continue;
        }

        let section_name = parse_section_name(trimmed);
        if !section_name.is_empty() {
            section = section_name;
            lines.push(TomlLine {
                text: raw_text.to_string(),
                eol: raw_eol.to_string(),
                kind: LineKind::Section,
                section: section.clone(),
                key: String::new(),
                value: String::new(),
                value_start: 0,
                value_end: 0,
            });
            continue;
        }

        let Some((key_literal, value_literal)) = match_entry(trimmed) else {
            lines.push(TomlLine {
                text: raw_text.to_string(),
                eol: raw_eol.to_string(),
                kind: LineKind::Text,
                section: section.clone(),
                key: String::new(),
                value: String::new(),
                value_start: 0,
                value_end: 0,
            });
            continue;
        };

        // 值 token 的位置在原始行里算：跳过键、等号与等号后的空白
        let offset = raw_text.find(trimmed).unwrap_or(0);
        let equals = trimmed.find('=').unwrap_or(0);
        let after_equals = &trimmed[equals + 1..];
        let value_text = after_equals.trim();
        let value_start = offset + equals + 1 + (after_equals.len() - after_equals.trim_start().len());
        lines.push(TomlLine {
            text: raw_text.to_string(),
            eol: raw_eol.to_string(),
            kind: LineKind::Entry,
            section: section.clone(),
            key: parse_toml_value(key_literal),
            value: parse_toml_value(value_literal),
            value_start,
            value_end: value_start + value_token_length(value_text),
        });

        for delimiter in MULTILINE_DELIMITERS {
            if value_literal.starts_with(delimiter) && !value_literal[delimiter.len()..].contains(delimiter) {
                multiline = delimiter.to_string();
            }
        }
    }

    TomlScan {
        lines,
        unterminated: !multiline.is_empty(),
    }
}

