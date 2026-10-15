// usage 字段的形状转换：三种协议的 usage 字段名不同，这里只做「换字段名」这一件事。
//
// 从 cc-switch 的 api-format-proxy 搬过来（`src/convert.rs` 里那三个函数），
// 只被 stream.rs 的「整块 JSON 展开成 SSE」用：展开时要拼出各协议自己的 usage 形状。
// 上游完整版把它们放在 convert.rs（那个文件里还有一套简化的格式转换，与 providers/ 下
// 真正的转换重复，所以没有整体搬过来，只取了这几个）

use serde_json::{json, Value};

pub(crate) fn openai_usage_to_anthropic(usage: Option<&Value>) -> Value {
    let Some(usage) = usage else {
        return json!({ "input_tokens": 0, "output_tokens": 0 });
    };
    let input = usage.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0);
    let output = usage.get("completion_tokens").and_then(Value::as_u64).unwrap_or(0);
    json!({ "input_tokens": input, "output_tokens": output })
}

pub(crate) fn openai_chat_usage_to_responses(usage: Option<&Value>) -> Value {
    let Some(usage) = usage else {
        return json!({ "input_tokens": 0, "output_tokens": 0, "total_tokens": 0 });
    };
    let input = usage.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0);
    let output = usage.get("completion_tokens").and_then(Value::as_u64).unwrap_or(0);
    json!({
        "input_tokens": input,
        "output_tokens": output,
        "total_tokens": input + output
    })
}

pub(crate) fn anthropic_usage_to_responses(usage: Option<&Value>) -> Value {
    let Some(usage) = usage else {
        return json!({ "input_tokens": 0, "output_tokens": 0, "total_tokens": 0 });
    };
    let input = usage.get("input_tokens").and_then(Value::as_u64).unwrap_or(0);
    let output = usage.get("output_tokens").and_then(Value::as_u64).unwrap_or(0);
    json!({
        "input_tokens": input,
        "output_tokens": output,
        "total_tokens": input + output
    })
}
