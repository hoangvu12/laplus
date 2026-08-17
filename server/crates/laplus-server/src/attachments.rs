//! Persistence and provider-side resolution for composer attachments.

use serde_json::Value;
use std::path::Path;

use crate::threads::PromptAttachment;

pub const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
pub(crate) const SUPPORTED_MIMES: [&str; 4] =
    ["image/png", "image/jpeg", "image/gif", "image/webp"];

pub fn resolve_all(
    values: &[Value],
    message_id: &str,
    preferences: &Path,
) -> Result<Vec<PromptAttachment>, String> {
    values
        .iter()
        .enumerate()
        .map(|(index, value)| resolve(value, message_id, index, preferences))
        .collect::<Result<Vec<_>, _>>()
        .map(|items| items.into_iter().flatten().collect())
}

pub fn resolve_all_required(
    values: &[Value], message_id: &str, preferences: &Path,
) -> Result<Vec<PromptAttachment>, String> {
    let resolved = resolve_all(values, message_id, preferences)?;
    if resolved.len() != values.len() {
        return Err("A stored image attachment could not be resolved.".into());
    }
    Ok(resolved)
}

/// Read one normalized image back for a provider that accepts inline data URLs.
pub(crate) fn data_url(attachment: &PromptAttachment) -> Result<String, String> {
    if !valid_id(&attachment.id) {
        return Err("The stored image attachment has an unsafe identity.".into());
    }
    let extension = extension(&attachment.mime)
        .ok_or_else(|| format!("Unsupported image attachment MIME type: {}.", attachment.mime))?;
    let expected_name = format!("{}.{}", attachment.id, extension);
    if attachment.path.file_name().and_then(|name| name.to_str()) != Some(&expected_name) {
        return Err("The stored image attachment path does not match its identity.".into());
    }
    let bytes = std::fs::read(&attachment.path)
        .map_err(|error| format!("The stored image attachment could not be read: {error}"))?;
    if bytes.is_empty() || bytes.len() as u64 != attachment.size_bytes {
        return Err("The stored image attachment content no longer matches its metadata.".into());
    }
    Ok(format!("data:{};base64,{}", attachment.mime, encode_base64(&bytes)))
}

fn encode_base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or_default();
        let third = chunk.get(2).copied().unwrap_or_default();
        output.push(ALPHABET[(first >> 2) as usize] as char);
        output.push(ALPHABET[(((first & 3) << 4) | (second >> 4)) as usize] as char);
        output.push(if chunk.len() > 1 { ALPHABET[(((second & 15) << 2) | (third >> 6)) as usize] as char } else { '=' });
        output.push(if chunk.len() > 2 { ALPHABET[(third & 63) as usize] as char } else { '=' });
    }
    output
}

fn resolve(
    value: &Value,
    message_id: &str,
    index: usize,
    preferences: &Path,
) -> Result<Option<PromptAttachment>, String> {
    if !valid_id(message_id) {
        return Err(
            "The image attachment cannot be stored under an unsafe message identity.".into(),
        );
    }
    let generated = format!("{message_id}-{index}");
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or(&generated);
    if !valid_id(id) {
        return Err("The image attachment has an unsafe identity.".into());
    }
    let name = value
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty() && name.len() <= 255)
        .ok_or_else(|| "The image attachment needs a valid name.".to_string())?
        .to_string();
    let mime = value
        .get("mimeType")
        .and_then(Value::as_str)
        .ok_or_else(|| "The image attachment needs a MIME type.".to_string())?;
    let extension = extension(mime)
        .ok_or_else(|| format!("Unsupported image attachment MIME type: {mime}."))?;
    let directory = preferences.join("attachments");
    let path = directory.join(format!("{id}.{extension}"));
    if let Some(data_url) = value.get("dataUrl").and_then(Value::as_str) {
        let (prefix, encoded) = data_url
            .split_once(',')
            .ok_or_else(|| "The image attachment has a malformed data URL.".to_string())?;
        if prefix != format!("data:{mime};base64") {
            return Err("The image attachment data URL does not match its MIME type.".into());
        }
        let bytes = decode_base64(encoded)
            .ok_or_else(|| "The image attachment has malformed base64 data.".to_string())?;
        if bytes.is_empty() {
            return Err("The image attachment is empty.".into());
        }
        if bytes.len() > MAX_IMAGE_BYTES {
            return Err("The image attachment exceeds the 10 MiB limit.".into());
        }
        if value.get("sizeBytes").and_then(Value::as_u64) != Some(bytes.len() as u64) {
            return Err("The image attachment byte count does not match its content.".into());
        }
        std::fs::create_dir_all(&directory).map_err(|error| {
            format!("The image attachment directory could not be created: {error}")
        })?;
        std::fs::write(&path, bytes)
            .map_err(|error| format!("The image attachment could not be stored: {error}"))?;
    }
    if !confined(&directory, &path) {
        return Err("The image attachment path is unsafe.".into());
    }
    if !path.is_file() {
        return if value.get("dataUrl").is_none() {
            Ok(None)
        } else {
            Err("The stored image attachment could not be resolved.".into())
        };
    }
    let size_bytes = std::fs::metadata(&path)
        .map_err(|error| format!("The stored image attachment could not be inspected: {error}"))?
        .len();
    Ok(Some(PromptAttachment {
        id: id.to_string(),
        mime: mime.to_string(),
        filename: name,
        size_bytes,
        path,
    }))
}

fn confined(directory: &Path, path: &Path) -> bool {
    path.parent() == Some(directory)
}

fn decode_base64(input: &str) -> Option<Vec<u8>> {
    let mut output = Vec::with_capacity(input.len() * 3 / 4);
    let mut chunk = [0u8; 4];
    let mut used = 0;
    let mut padded = false;
    for byte in input.bytes() {
        if byte.is_ascii_whitespace() {
            return None;
        }
        if padded {
            return None;
        }
        chunk[used] = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => 64,
            _ => return None,
        };
        used += 1;
        if used == 4 {
            if chunk[0] == 64 || chunk[1] == 64 || (chunk[2] == 64 && chunk[3] != 64) {
                return None;
            }
            if (chunk[2] == 64 && chunk[1] & 0x0f != 0)
                || (chunk[3] == 64 && chunk[2] != 64 && chunk[2] & 0x03 != 0)
            {
                return None;
            }
            output.push((chunk[0] << 2) | (chunk[1] >> 4));
            if chunk[2] != 64 {
                output.push((chunk[1] << 4) | (chunk[2] >> 2));
            }
            if chunk[3] != 64 {
                output.push((chunk[2] << 6) | chunk[3]);
            }
            padded = chunk[2] == 64 || chunk[3] == 64;
            used = 0;
        }
    }
    (used == 0).then_some(output)
}

pub(crate) fn encode_base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        output.push(TABLE[(chunk[0] >> 2) as usize] as char);
        output.push(TABLE[((chunk[0] & 0x03) << 4 | chunk.get(1).copied().unwrap_or(0) >> 4) as usize] as char);
        match chunk.get(1) {
            Some(second) => output.push(TABLE[((second & 0x0f) << 2 | chunk.get(2).copied().unwrap_or(0) >> 6) as usize] as char),
            None => output.push('='),
        }
        match chunk.get(2) {
            Some(third) => output.push(TABLE[(third & 0x3f) as usize] as char),
            None => output.push('='),
        }
    }
    output
}

pub(crate) fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
}

pub(crate) fn extension(mime: &str) -> Option<&'static str> {
    match mime {
        "image/png" => Some("png"),
        "image/jpeg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn upload(mime: &str, size: usize, data: &str) -> Value {
        json!({"type":"image","name":"image","mimeType":mime,"sizeBytes":size,"dataUrl":format!("data:{mime};base64,{data}")})
    }

    #[test]
    fn supported_images_are_normalized_and_stored() {
        for (mime, suffix) in [
            ("image/png", "png"),
            ("image/jpeg", "jpg"),
            ("image/gif", "gif"),
            ("image/webp", "webp"),
        ] {
            let root = tempfile::tempdir().unwrap();
            let attachment = resolve_all(&[upload(mime, 2, "aGk=")], "message", root.path())
                .unwrap()
                .remove(0);
            assert_eq!(attachment.id, "message-0");
            assert_eq!(attachment.mime, mime);
            assert_eq!(attachment.size_bytes, 2);
            assert_eq!(
                attachment.path.file_name().unwrap().to_string_lossy(),
                format!("message-0.{suffix}")
            );
        }
    }

    #[test]
    fn invalid_uploads_are_refused() {
        let root = tempfile::tempdir().unwrap();
        for candidate in [
            upload("image/png", 0, ""),
            upload("image/png", 2, "%%%="),
            upload("image/png", 1, "aG=="),
            upload("image/svg+xml", 2, "aGk="),
            upload("image/png", 3, "aGk="),
        ] {
            assert!(resolve_all(&[candidate], "message", root.path()).is_err());
        }
        let unsafe_reference =
            json!({"id":"../escape","name":"x.png","mimeType":"image/png","sizeBytes":2});
        assert!(resolve_all(&[unsafe_reference], "message", root.path()).is_err());
        assert!(resolve_all(&[upload("image/png", 2, "aGk=")], "../message", root.path()).is_err());
    }

    #[test]
    fn the_ten_mib_decoded_limit_is_exact() {
        let root = tempfile::tempdir().unwrap();
        let encoded = |size| base64_for_zeroes(size);
        assert!(resolve_all(
            &[upload(
                "image/png",
                MAX_IMAGE_BYTES,
                &encoded(MAX_IMAGE_BYTES)
            )],
            "at-limit",
            root.path()
        )
        .is_ok());
        assert!(resolve_all(
            &[upload(
                "image/png",
                MAX_IMAGE_BYTES + 1,
                &encoded(MAX_IMAGE_BYTES + 1)
            )],
            "over-limit",
            root.path()
        )
        .is_err());
    }

    fn base64_for_zeroes(size: usize) -> String {
        let full = size / 3;
        let rem = size % 3;
        let mut value = "AAAA".repeat(full);
        if rem == 1 {
            value.push_str("AA==");
        } else if rem == 2 {
            value.push_str("AAA=");
        }
        value
    }

    #[test]
    fn persistence_failures_are_refused() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("attachments"), b"not a directory").unwrap();
        assert!(resolve_all(&[upload("image/png", 2, "aGk=")], "message", root.path()).is_err());
    }
}
