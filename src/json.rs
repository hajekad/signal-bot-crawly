/// Escape a string for inclusion in a JSON string value.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

/// Extract a JSON string value for the given key.
/// Searches for `"key": "value"` or `"key":"value"` and returns the unescaped value.
/// Verifies the key is standalone (not a substring of another key like "groupId" matching "id").
pub fn extract_string(json: &str, key: &str) -> Option<String> {
    let pattern = format!("\"{}\"", key);
    let mut search_from = 0;
    loop {
        let idx = json[search_from..].find(&pattern)?;
        let abs_idx = search_from + idx;
        // Verify this is a standalone key, not a substring of another key
        if abs_idx == 0 || !json.as_bytes()[abs_idx - 1].is_ascii_alphanumeric() {
            let after_key = &json[abs_idx + pattern.len()..];
            let after_colon = after_key.trim_start().strip_prefix(':')?;
            let trimmed = after_colon.trim_start();
            if trimmed.starts_with('"') {
                return extract_json_string_at(trimmed);
            }
        }
        search_from = abs_idx + 1;
    }
}

/// Extract a JSON number value for the given key.
/// Verifies the key is standalone (not a substring of another key).
pub fn extract_number(json: &str, key: &str) -> Option<i64> {
    let pattern = format!("\"{}\"", key);
    let mut search_from = 0;
    loop {
        let idx = json[search_from..].find(&pattern)?;
        let abs_idx = search_from + idx;
        // Verify this is a standalone key, not a substring of another key
        if abs_idx == 0 || !json.as_bytes()[abs_idx - 1].is_ascii_alphanumeric() {
            let after_key = &json[abs_idx + pattern.len()..];
            if let Some(after_colon) = after_key.trim_start().strip_prefix(':') {
                let trimmed = after_colon.trim_start();
                let end = trimmed.find(|c: char| !c.is_ascii_digit() && c != '-').unwrap_or(trimmed.len());
                if end > 0 {
                    if let Ok(n) = trimmed[..end].parse() {
                        return Some(n);
                    }
                }
            }
        }
        search_from = abs_idx + 1;
    }
}

/// Extract a JSON string starting at the opening quote.
fn extract_json_string_at(s: &str) -> Option<String> {
    if !s.starts_with('"') {
        return None;
    }

    let mut result = String::new();
    let mut chars = s[1..].chars();
    loop {
        let c = chars.next()?;
        match c {
            '"' => return Some(result),
            '\\' => {
                let escaped = chars.next()?;
                match escaped {
                    '"' => result.push('"'),
                    '\\' => result.push('\\'),
                    '/' => result.push('/'),
                    'n' => result.push('\n'),
                    'r' => result.push('\r'),
                    't' => result.push('\t'),
                    'u' => {
                        let mut hex = String::with_capacity(4);
                        for _ in 0..4 {
                            hex.push(chars.next()?);
                        }
                        let cp = u32::from_str_radix(&hex, 16).ok()?;
                        if (0xD800..=0xDBFF).contains(&cp) {
                            // High surrogate — expect \uDCxx low surrogate
                            if chars.next() == Some('\\') && chars.next() == Some('u') {
                                let mut hex2 = String::with_capacity(4);
                                for _ in 0..4 {
                                    hex2.push(chars.next()?);
                                }
                                let cp2 = u32::from_str_radix(&hex2, 16).ok()?;
                                if (0xDC00..=0xDFFF).contains(&cp2) {
                                    let combined = 0x10000 + ((cp - 0xD800) << 10) + (cp2 - 0xDC00);
                                    if let Some(c) = char::from_u32(combined) {
                                        result.push(c);
                                    }
                                }
                            }
                        } else if let Some(c) = char::from_u32(cp) {
                            result.push(c);
                        }
                    }
                    _ => {
                        result.push('\\');
                        result.push(escaped);
                    }
                }
            }
            _ => result.push(c),
        }
    }
}

/// Extract all objects from a JSON array. Returns the raw JSON strings of each object.
pub fn extract_array_objects(json: &str) -> Vec<String> {
    let mut objects = Vec::new();

    // Find the opening bracket
    let start = match json.find('[') {
        Some(i) => i + 1,
        None => return objects,
    };

    let json_bytes = json.as_bytes();
    let len = json_bytes.len();
    let mut i = start;

    while i < len {
        // Skip whitespace and commas
        match json_bytes[i] {
            b' ' | b'\t' | b'\n' | b'\r' | b',' => {
                i += 1;
                continue;
            }
            b']' => break,
            b'{' => {
                // Find matching closing brace
                let obj_start = i;
                let mut depth = 0;
                let mut in_string = false;
                let mut escape_next = false;

                while i < len {
                    if escape_next {
                        escape_next = false;
                        i += 1;
                        continue;
                    }

                    match json_bytes[i] {
                        b'\\' if in_string => escape_next = true,
                        b'"' => in_string = !in_string,
                        b'{' if !in_string => depth += 1,
                        b'}' if !in_string => {
                            depth -= 1;
                            if depth == 0 {
                                objects.push(json[obj_start..=i].to_string());
                                i += 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                    i += 1;
                }
            }
            _ => {
                i += 1;
            }
        }
    }

    objects
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape() {
        assert_eq!(escape("hello"), "hello");
        assert_eq!(escape("he\"llo"), "he\\\"llo");
        assert_eq!(escape("line1\nline2"), "line1\\nline2");
    }

    #[test]
    fn test_extract_string() {
        let json = r#"{"name": "Family Chat", "id": "group.abc123"}"#;
        assert_eq!(extract_string(json, "name"), Some("Family Chat".to_string()));
        assert_eq!(extract_string(json, "id"), Some("group.abc123".to_string()));
        assert_eq!(extract_string(json, "missing"), None);
    }

    #[test]
    fn test_extract_string_with_escapes() {
        let json = r#"{"message": "hello \"world\"\nnewline"}"#;
        assert_eq!(
            extract_string(json, "message"),
            Some("hello \"world\"\nnewline".to_string())
        );
    }

    #[test]
    fn test_extract_number() {
        let json = r#"{"timestamp": 1612041718367, "other": 42}"#;
        assert_eq!(extract_number(json, "timestamp"), Some(1612041718367));
        assert_eq!(extract_number(json, "other"), Some(42));
    }

    #[test]
    fn test_extract_array_objects() {
        let json = r#"[{"a": 1}, {"b": 2}]"#;
        let objects = extract_array_objects(json);
        assert_eq!(objects.len(), 2);
        assert_eq!(objects[0], r#"{"a": 1}"#);
        assert_eq!(objects[1], r#"{"b": 2}"#);
    }

    #[test]
    fn test_extract_nested_objects() {
        let json = r#"[{"envelope": {"dataMessage": {"message": "hi"}}}]"#;
        let objects = extract_array_objects(json);
        assert_eq!(objects.len(), 1);
        assert_eq!(
            extract_string(&objects[0], "message"),
            Some("hi".to_string())
        );
    }

    #[test]
    fn test_escape_control_chars() {
        assert_eq!(escape("\t"), "\\t");
        assert_eq!(escape("\r"), "\\r");
        assert_eq!(escape("\\"), "\\\\");
    }

    #[test]
    fn test_escape_low_control_chars() {
        let s = String::from('\x01');
        assert_eq!(escape(&s), "\\u0001");
    }

    #[test]
    fn test_extract_string_no_spaces() {
        let json = r#"{"key":"value"}"#;
        assert_eq!(extract_string(json, "key"), Some("value".to_string()));
    }

    #[test]
    fn test_extract_string_with_unicode_escape() {
        let json = r#"{"msg": "hello \u0041"}"#;
        assert_eq!(extract_string(json, "msg"), Some("hello A".to_string()));
    }

    #[test]
    fn test_extract_string_value_is_number_returns_none() {
        let json = r#"{"count": 42}"#;
        assert_eq!(extract_string(json, "count"), None);
    }

    #[test]
    fn test_extract_number_negative() {
        let json = r#"{"offset": -100}"#;
        assert_eq!(extract_number(json, "offset"), Some(-100));
    }

    #[test]
    fn test_extract_number_missing() {
        let json = r#"{"name": "test"}"#;
        assert_eq!(extract_number(json, "count"), None);
    }

    #[test]
    fn test_extract_array_objects_empty() {
        assert_eq!(extract_array_objects("[]").len(), 0);
    }

    #[test]
    fn test_extract_array_objects_no_array() {
        assert_eq!(extract_array_objects("not json").len(), 0);
    }

    #[test]
    fn test_extract_array_objects_with_strings_containing_braces() {
        let json = r#"[{"msg": "use { and } in code"}]"#;
        let objects = extract_array_objects(json);
        assert_eq!(objects.len(), 1);
        assert_eq!(
            extract_string(&objects[0], "msg"),
            Some("use { and } in code".to_string())
        );
    }

    #[test]
    fn test_extract_string_finds_first_occurrence() {
        // When key appears in nested context, extract_string finds the first one
        let json = r#"{"outer": "first", "nested": {"outer": "second"}}"#;
        assert_eq!(extract_string(json, "outer"), Some("first".to_string()));
    }

    #[test]
    fn test_extract_array_deeply_nested() {
        let json = r#"[{"a":{"b":{"c":{"d":"deep"}}}}]"#;
        let objects = extract_array_objects(json);
        assert_eq!(objects.len(), 1);
        assert_eq!(extract_string(&objects[0], "d"), Some("deep".to_string()));
    }

    #[test]
    fn test_escape_roundtrip() {
        let original = "Hello \"world\"\nNew line\tTab\\Backslash";
        let escaped = escape(original);
        // Verify the escaped version would be valid inside JSON quotes
        let json = format!(r#"{{"test": "{}"}}"#, escaped);
        let extracted = extract_string(&json, "test").unwrap();
        assert_eq!(extracted, original);
    }

    #[test]
    fn test_extract_string_empty_value() {
        let json = r#"{"key": ""}"#;
        assert_eq!(extract_string(json, "key"), Some("".to_string()));
    }

    #[test]
    fn test_extract_array_objects_three_items() {
        let json = r#"[{"a":1},{"b":2},{"c":3}]"#;
        let objects = extract_array_objects(json);
        assert_eq!(objects.len(), 3);
    }

    #[test]
    fn test_full_signal_receive_response() {
        // Realistic full signal-cli response
        let json = r#"[{"envelope":{"source":"+1987654321","sourceNumber":"+1987654321","sourceDevice":1,"timestamp":1612041718367,"dataMessage":{"timestamp":1612041718367,"message":"Hello there!","expiresInSeconds":0,"viewOnce":false,"groupInfo":{"groupId":"group.abc123","type":"DELIVER"}}}},{"envelope":{"source":"+1555000111","sourceDevice":1,"timestamp":1612041800000,"dataMessage":{"timestamp":1612041800000,"message":"Anyone free for lunch?","groupInfo":{"groupId":"group.abc123","type":"DELIVER"}}}}]"#;

        let objects = extract_array_objects(json);
        assert_eq!(objects.len(), 2);

        // First message
        assert_eq!(extract_string(&objects[0], "message"), Some("Hello there!".to_string()));
        assert_eq!(extract_string(&objects[0], "source"), Some("+1987654321".to_string()));
        assert_eq!(extract_number(&objects[0], "timestamp"), Some(1612041718367));
        assert_eq!(extract_string(&objects[0], "groupId"), Some("group.abc123".to_string()));

        // Second message
        assert_eq!(extract_string(&objects[1], "message"), Some("Anyone free for lunch?".to_string()));
        assert_eq!(extract_string(&objects[1], "source"), Some("+1555000111".to_string()));
    }

    #[test]
    fn test_extract_string_key_not_substring_match() {
        // "id" must not match "groupId"
        let json = r#"{"groupId": "group.abc123", "id": "standalone-id"}"#;
        assert_eq!(extract_string(json, "id"), Some("standalone-id".to_string()));
    }

    #[test]
    fn test_extract_number_key_not_substring_match() {
        // "id" must not match "parentId"
        let json = r#"{"parentId": 999, "id": 42}"#;
        assert_eq!(extract_number(json, "id"), Some(42));
    }

    #[test]
    fn test_extract_string_surrogate_pair_emoji() {
        // \uD83D\uDE00 = U+1F600 grinning face
        let json = r#"{"emoji": "\uD83D\uDE00"}"#;
        let result = extract_string(json, "emoji").unwrap();
        assert_eq!(result, "\u{1F600}");
    }

    #[test]
    fn test_extract_string_surrogate_pair_mixed() {
        // Mix of surrogate pair emoji and normal text
        let json = r#"{"msg": "Hello \uD83D\uDE00 world"}"#;
        let result = extract_string(json, "msg").unwrap();
        assert_eq!(result, "Hello \u{1F600} world");
    }

    #[test]
    fn test_extract_string_surrogate_pair_multiple() {
        // Two surrogate pair emojis back to back
        let json = r#"{"msg": "\uD83D\uDE00\uD83D\uDE01"}"#;
        let result = extract_string(json, "msg").unwrap();
        assert_eq!(result, "\u{1F600}\u{1F601}");
    }

    #[test]
    fn test_full_ollama_response() {
        let json = r#"{"model":"llama3.2","message":{"role":"assistant","content":"Here is the summary:\n\u2022 Alice proposed a meeting\n\u2022 Bob confirmed availability"},"done":true,"done_reason":"stop","total_duration":2145000000,"eval_count":67,"eval_duration":1850000000}"#;

        assert_eq!(extract_string(json, "model"), Some("llama3.2".to_string()));
        assert_eq!(extract_string(json, "role"), Some("assistant".to_string()));
        let content = extract_string(json, "content").unwrap();
        assert!(content.contains("summary"));
        assert!(content.contains("Alice"));
        assert!(content.contains("Bob"));
    }
}
