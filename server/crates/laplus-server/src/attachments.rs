//! Persistence and provider-side resolution for composer attachments.

use std::path::Path;
use serde_json::Value;

use crate::threads::PromptAttachment;

pub fn resolve_all(values: &[Value], message_id: &str) -> Vec<PromptAttachment> {
    values.iter().enumerate().filter_map(|(index, value)| resolve(value, message_id, index)).collect()
}

fn resolve(value: &Value, message_id: &str, index: usize) -> Option<PromptAttachment> {
    let generated = format!("{}-{index}", message_id.chars().filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_')).collect::<String>());
    let id = value.get("id").and_then(Value::as_str).unwrap_or(&generated);
    if id.is_empty() || !id.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_')) { return None; }
    let name = value.get("name")?.as_str()?.to_string();
    let mime = value.get("mimeType")?.as_str()?;
    let extension = match mime { "image/png" => "png", "image/jpeg" => "jpg", "image/gif" => "gif", "image/webp" => "webp", _ => return None };
    let directory = crate::config::data_dir().join("attachments");
    let path = directory.join(format!("{id}.{extension}"));
    if let Some(data_url) = value.get("dataUrl").and_then(Value::as_str) {
        let (prefix, encoded) = data_url.split_once(',')?;
        if prefix != format!("data:{mime};base64") { return None; }
        let bytes = decode_base64(encoded)?;
        if value.get("sizeBytes").and_then(Value::as_u64) != Some(bytes.len() as u64) { return None; }
        std::fs::create_dir_all(&directory).ok()?;
        std::fs::write(&path, bytes).ok()?;
    }
    confined(&directory, &path).then_some(PromptAttachment { mime: mime.to_string(), filename: name, path })
        .filter(|attachment| attachment.path.is_file())
}

fn confined(directory: &Path, path: &Path) -> bool { path.parent() == Some(directory) }

fn decode_base64(input: &str) -> Option<Vec<u8>> {
    let mut output = Vec::with_capacity(input.len() * 3 / 4);
    let mut chunk = [0u8; 4];
    let mut used = 0;
    for byte in input.bytes().filter(|byte| !byte.is_ascii_whitespace()) {
        chunk[used] = match byte { b'A'..=b'Z' => byte-b'A', b'a'..=b'z' => byte-b'a'+26, b'0'..=b'9' => byte-b'0'+52, b'+' => 62, b'/' => 63, b'=' => 64, _ => return None };
        used += 1;
        if used == 4 {
            if chunk[0] == 64 || chunk[1] == 64 || (chunk[2] == 64 && chunk[3] != 64) { return None; }
            output.push((chunk[0] << 2) | (chunk[1] >> 4));
            if chunk[2] != 64 { output.push((chunk[1] << 4) | (chunk[2] >> 2)); }
            if chunk[3] != 64 { output.push((chunk[2] << 6) | chunk[3]); }
            used = 0;
        }
    }
    (used == 0).then_some(output)
}
