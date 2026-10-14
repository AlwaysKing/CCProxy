// 只改 JSON 文本里目标字段的值，其余内容（缩进、键序、\u 转义、行尾、权限、插件、
// hooks）一个字节都不碰。
// 不用「parse 再 stringify」是因为那会把整个文件重排版——claude 的 settings.json
// 里还有用户自己的权限、插件、hooks，改两个 env 值不该让全文跟着重排。
// 合法性由解析保证：进来先 parse 一次，拼完再 parse 一次校验。
// 万一下标算歪了，宁可抛错让调用方放弃写入，也不交出一份坏配置。

use serde_json::Value;

pub struct JsonMember {
    pub name: String,
    pub key_start: usize,
    pub value_start: usize,
    pub value_end: usize,
}

pub struct JsonObject {
    pub members: Vec<JsonMember>,
    pub close_index: usize,
}

pub struct JsonArray {
    pub items: Vec<(usize, usize)>,
    pub close_index: usize,
}

pub struct Edit {
    pub start: usize,
    pub end: usize,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct JsonEdit {
    pub text: String,
    pub changed: bool,
}

fn is_whitespace(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\r' | b'\n')
}

fn skip_whitespace(bytes: &[u8], index: usize) -> usize {
    let mut cursor = index;
    while cursor < bytes.len() && is_whitespace(bytes[cursor]) {
        cursor += 1;
    }
    cursor
}

// 从开引号扫到闭合引号之后；没闭合返回 None
fn scan_string_literal(bytes: &[u8], index: usize) -> Option<usize> {
    let mut cursor = index + 1;
    while cursor < bytes.len() {
        if bytes[cursor] == b'\\' {
            cursor += 2;
            continue;
        }
        if bytes[cursor] == b'"' {
            return Some(cursor + 1);
        }
        cursor += 1;
    }
    None
}

// 扫过任意一个值（含嵌套的对象/数组），返回它之后的下标；结构不对返回 None。
// 按深度配平括号，字符串整段跳过——字符串里的括号不算数
fn scan_value(bytes: &[u8], index: usize) -> Option<usize> {
    let Some(&character) = bytes.get(index) else {
        return None;
    };
    if character == b'"' {
        return scan_string_literal(bytes, index);
    }
    if character == b'{' || character == b'[' {
        let mut depth = 0i64;
        let mut cursor = index;
        while cursor < bytes.len() {
            let current = bytes[cursor];
            if current == b'"' {
                cursor = scan_string_literal(bytes, cursor)?;
                continue;
            }
            if current == b'{' || current == b'[' {
                depth += 1;
            } else if current == b'}' || current == b']' {
                depth -= 1;
                if depth == 0 {
                    return Some(cursor + 1);
                }
            }
            cursor += 1;
        }
        return None;
    }
    // 数字 / true / false / null：扫到分隔符为止
    let mut cursor = index;
    while cursor < bytes.len() && !matches!(bytes[cursor], b',' | b'}' | b']' | b' ' | b'\t' | b'\r' | b'\n') {
        cursor += 1;
    }
    Some(cursor)
}

// 读一个对象，给出它的每个直接成员与闭合花括号的位置；读不成给出 None
pub fn read_json_object(text: &str, object_start: usize) -> Option<JsonObject> {
    let bytes = text.as_bytes();
    if bytes.get(object_start) != Some(&b'{') {
        return None;
    }
    let mut members = Vec::new();
    let mut cursor = skip_whitespace(bytes, object_start + 1);
    if bytes.get(cursor) == Some(&b'}') {
        return Some(JsonObject { members, close_index: cursor });
    }

    while cursor < bytes.len() {
        let key_start = cursor;
        if bytes.get(key_start) != Some(&b'"') {
            return None;
        }
        let key_end = scan_string_literal(bytes, key_start)?;
        let name: String = serde_json::from_str(&text[key_start..key_end]).ok()?;
        cursor = skip_whitespace(bytes, key_end);
        if bytes.get(cursor) != Some(&b':') {
            return None;
        }
        let value_start = skip_whitespace(bytes, cursor + 1);
        let value_end = scan_value(bytes, value_start)?;
        members.push(JsonMember { name, key_start, value_start, value_end });
        cursor = skip_whitespace(bytes, value_end);
        if bytes.get(cursor) == Some(&b'}') {
            return Some(JsonObject { members, close_index: cursor });
        }
        if bytes.get(cursor) != Some(&b',') {
            return None;
        }
        cursor = skip_whitespace(bytes, cursor + 1);
    }
    None
}

// 读一个数组，给出每个元素值的下标范围与闭合方括号的位置；读不成给出 None
pub fn read_json_array(text: &str, array_start: usize) -> Option<JsonArray> {
    let bytes = text.as_bytes();
    if bytes.get(array_start) != Some(&b'[') {
        return None;
    }
    let mut items = Vec::new();
    let mut cursor = skip_whitespace(bytes, array_start + 1);
    if bytes.get(cursor) == Some(&b']') {
        return Some(JsonArray { items, close_index: cursor });
    }

    while cursor < bytes.len() {
        let start = cursor;
        let end = scan_value(bytes, start)?;
        items.push((start, end));
        cursor = skip_whitespace(bytes, end);
        if bytes.get(cursor) == Some(&b']') {
            return Some(JsonArray { items, close_index: cursor });
        }
        if bytes.get(cursor) != Some(&b',') {
            return None;
        }
        cursor = skip_whitespace(bytes, cursor + 1);
    }
    None
}

// index 所在行的缩进（新插入的键照它对齐；已有内容一概不动）
pub fn indent_at(text: &str, index: usize) -> String {
    let bytes = text.as_bytes();
    if bytes.is_empty() {
        return String::new();
    }
    let from = if index > 0 { index - 1 } else { 0 };
    let from = from.min(bytes.len() - 1);
    let mut line_start = 0;
    let mut cursor = from;
    loop {
        if bytes[cursor] == b'\n' {
            line_start = cursor + 1;
            break;
        }
        if cursor == 0 {
            break;
        }
        cursor -= 1;
    }
    let mut width = 0;
    while line_start + width < bytes.len() && (bytes[line_start + width] == b' ' || bytes[line_start + width] == b'\t') {
        width += 1;
    }
    text[line_start..line_start + width].to_string()
}

// 文件惯用的缩进单位：所有行里最短的那一段正缩进。缩进是 4 空格的文件就还给 4 空格，
// 全压成一行的文件认不出来，退回两空格
pub fn detect_indent_unit(text: &str) -> String {
    let mut unit = String::new();
    for line in text.split('\n') {
        let width = line.bytes().take_while(|byte| *byte == b' ' || *byte == b'\t').count();
        if width > 0 && (unit.is_empty() || width < unit.len()) {
            unit = line[..width].to_string();
        }
    }
    if unit.is_empty() {
        "  ".to_string()
    } else {
        unit
    }
}

// 从后往前拼，前面的下标才不会被已经改过的内容顶偏
fn apply_edits(text: &str, edits: &[Edit]) -> String {
    let mut ordered: Vec<&Edit> = edits.iter().collect();
    ordered.sort_by(|a, b| b.start.cmp(&a.start));
    let mut result = text.to_string();
    for edit in ordered {
        result = format!("{}{}{}", &result[..edit.start], edit.text, &result[edit.end..]);
    }
    result
}

// 整块新建一个对象（目标字段缺失或不是对象时用）
fn build_object_text(values: &[(String, Value)], indent: &str, unit: &str) -> String {
    let lines: Vec<String> = values
        .iter()
        .map(|(key, value)| {
            format!(
                "{indent}{unit}{}: {}",
                serde_json::to_string(key).unwrap_or_default(),
                serde_json::to_string(value).unwrap_or_default()
            )
        })
        .collect();
    format!("{{\n{}\n{indent}}}", lines.join(",\n"))
}

// 报错都带上「为免写坏已放弃」：调用方只写文件名，用户看到的是一句完整的话
fn parse_or_throw(json_text: &str) -> Result<Value, String> {
    serde_json::from_str(json_text).map_err(|error| format!("不是合法 JSON，为免写坏已放弃：{error}"))
}

// 拼回去再 parse 一遍：下标算错了宁可抛错，也不要交出坏配置
pub fn finish(text: &str, edits: Vec<Edit>) -> Result<JsonEdit, String> {
    if edits.is_empty() {
        return Ok(JsonEdit { text: text.to_string(), changed: false });
    }
    let result = apply_edits(text, &edits);
    if let Err(error) = serde_json::from_str::<Value>(&result) {
        return Err(format!("按目标字段改写后不是合法 JSON，为免写坏已放弃：{error}"));
    }
    Ok(JsonEdit { text: result, changed: true })
}

// 把 json_text 里 object_name（空串表示根对象）那个对象下的若干键设成新值。
// 只替换目标值的字节区间，值本来就等于目标值就不动它。结构不是预期的样子就抛错
pub fn set_object_values(json_text: &str, object_name: &str, values: &[(String, Value)]) -> Result<JsonEdit, String> {
    let text = json_text.to_string();
    let parsed = parse_or_throw(&text)?;
    if !parsed.is_object() {
        return Err("根节点不是对象，为免写坏已放弃".to_string());
    }

    let root_start = skip_whitespace(text.as_bytes(), 0);
    let Some(root) = read_json_object(&text, root_start) else {
        return Err("结构看不懂，为免写坏已放弃".to_string());
    };
    let unit = detect_indent_unit(&text);
    let mut edits: Vec<Edit> = Vec::new();

    let mut container = root;
    let mut container_start = root_start;

    if !object_name.is_empty() {
        let member = container.members.iter().find(|item| item.name == object_name);
        let existing = member.and_then(|item| read_json_object(&text, item.value_start));
        match existing {
            Some(existing) => {
                container_start = member.map(|item| item.value_start).unwrap_or(root_start);
                container = existing;
            }
            None => {
                // 整个对象要么没有、要么不是对象：这一块本来就归我们写，一次换掉
                let indent = if container.members.is_empty() {
                    format!("{}{}", indent_at(&text, container.close_index), unit)
                } else {
                    indent_at(&text, container.members[0].key_start)
                };
                let block = build_object_text(values, &indent, &unit);
                let name_literal = serde_json::to_string(object_name).unwrap_or_default();
                match member {
                    Some(member) => edits.push(Edit {
                        start: member.value_start,
                        end: member.value_end,
                        text: block,
                    }),
                    None if !container.members.is_empty() => {
                        let anchor = container.members[container.members.len() - 1].value_end;
                        edits.push(Edit {
                            start: anchor,
                            end: anchor,
                            text: format!(",\n{indent}{name_literal}: {block}"),
                        });
                    }
                    None => {
                        let close_indent = indent_at(&text, container.close_index);
                        edits.push(Edit {
                            start: root_start + 1,
                            end: root_start + 1,
                            text: format!("\n{indent}{name_literal}: {block}\n{close_indent}"),
                        });
                    }
                }
                return finish(&text, edits);
            }
        }
    }

    plan_members_at(&text, &container, container_start, values, &unit, &mut edits);
    finish(&text, edits)
}

// 只算编辑不落文本：catalog 要把多个条目的改动和新增条目攒成一份编辑表一次提交，
// 全文只 parse 校验一遍。container_start 是容器对象 '{' 的下标（空对象补键时用）
pub fn plan_members_at(
    text: &str,
    container: &JsonObject,
    container_start: usize,
    values: &[(String, Value)],
    unit: &str,
    edits: &mut Vec<Edit>,
) {
    let key_indent = if container.members.is_empty() {
        format!("{}{}", indent_at(text, container.close_index), unit)
    } else {
        indent_at(text, container.members[0].key_start)
    };

    // 缺的键一次插完：分两次插会在同一个位置上打架
    let missing: Vec<&(String, Value)> = values
        .iter()
        .filter(|(key, _)| !container.members.iter().any(|item| item.name == *key))
        .collect();
    if !missing.is_empty() {
        let block = missing
            .iter()
            .map(|(key, value)| {
                format!(
                    "{key_indent}{}: {}",
                    serde_json::to_string(key).unwrap_or_default(),
                    serde_json::to_string(value).unwrap_or_default()
                )
            })
            .collect::<Vec<String>>()
            .join(",\n");
        if container.members.is_empty() {
            let close_indent = indent_at(text, container.close_index);
            edits.push(Edit {
                start: container_start + 1,
                end: container_start + 1,
                text: format!("\n{block}\n{close_indent}"),
            });
        } else {
            let anchor = container.members[container.members.len() - 1].value_end;
            edits.push(Edit {
                start: anchor,
                end: anchor,
                text: format!(",\n{block}"),
            });
        }
    }

    for (key, value) in values {
        let Some(member) = container.members.iter().find(|item| item.name == *key) else {
            continue;
        };
        // 值本来就是目标值就别动它，连一个字节都不碰（转义写法不同但值相同也算不碰）
        if let Ok(current) = serde_json::from_str::<Value>(&text[member.value_start..member.value_end]) {
            if current == *value {
                continue;
            }
        }
        edits.push(Edit {
            start: member.value_start,
            end: member.value_end,
            text: serde_json::to_string(value).unwrap_or_default(),
        });
    }
}


