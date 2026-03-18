use crate::http;
use crate::json;

/// A Signal group with its display name and API group ID.
#[derive(Debug, Clone)]
pub struct Group {
    pub name: String,
    pub id: String,
    pub internal_id: String,
}

/// A received message from Signal.
#[derive(Debug)]
pub struct Message {
    pub sender: String,
    pub sender_name: Option<String>,
    pub text: String,
    pub timestamp: i64,
    pub group_id: Option<String>,
    pub mentions_bot: bool,
}

/// Fetch all groups for the registered phone number.
pub fn list_groups(host: &str, port: u16, phone: &str) -> Result<Vec<Group>, String> {
    let path = format!("/v1/groups/{}", phone);
    let (status, body) = http::http_get(host, port, &path)?;

    if status != 200 {
        return Err(format!("List groups failed (HTTP {}): {}", status, body));
    }

    Ok(parse_groups(&body))
}

/// Receive (consume) all pending messages for the phone number.
/// `bot_id` is the identifier (UUID or phone) used to detect bot mentions.
pub fn receive_messages(host: &str, port: u16, phone: &str, bot_id: &str) -> Result<Vec<Message>, String> {
    let path = format!("/v1/receive/{}", phone);
    let (status, body) = http::http_get(host, port, &path)?;

    if status != 200 {
        return Err(format!("Receive messages failed (HTTP {}): {}", status, body));
    }

    Ok(parse_messages(&body, bot_id))
}

/// Parse groups from JSON response body.
pub fn parse_groups(body: &str) -> Vec<Group> {
    let objects = json::extract_array_objects(body);
    let mut groups = Vec::new();
    for obj in &objects {
        if let (Some(name), Some(id)) = (json::extract_string(obj, "name"), json::extract_string(obj, "id")) {
            let internal_id = json::extract_string(obj, "internal_id").unwrap_or_default();
            groups.push(Group { name, id, internal_id });
        }
    }
    groups
}

/// Parse messages from JSON response body.
/// `bot_identifiers` are strings (phone, UUID, username) to match in mentions.
pub fn parse_messages(body: &str, bot_id: &str) -> Vec<Message> {
    let envelopes = json::extract_array_objects(body);
    let mut messages = Vec::new();
    for envelope_obj in &envelopes {
        let text = match json::extract_string(envelope_obj, "message") {
            Some(t) if !t.is_empty() => t,
            _ => continue,
        };
        let sender = json::extract_string(envelope_obj, "source")
            .or_else(|| json::extract_string(envelope_obj, "sourceNumber"))
            .unwrap_or_else(|| "unknown".to_string());
        let sender_name = json::extract_string(envelope_obj, "sourceName");
        let timestamp = json::extract_number(envelope_obj, "timestamp").unwrap_or(0);
        let group_id = json::extract_string(envelope_obj, "groupId");
        let mentions_bot = has_bot_mention(envelope_obj, bot_id);
        messages.push(Message { sender, sender_name, text, timestamp, group_id, mentions_bot });
    }
    messages
}

/// Look up the bot's UUID from the identities endpoint.
pub fn get_bot_uuid(host: &str, port: u16, phone: &str) -> Result<String, String> {
    let path = format!("/v1/identities/{}", phone);
    let (status, body) = http::http_get(host, port, &path)?;

    if status != 200 {
        return Err(format!("Get identities failed (HTTP {}): {}", status, body));
    }

    // Find the identity entry whose "number" matches the bot's phone
    let objects = json::extract_array_objects(&body);
    for obj in &objects {
        if let Some(number) = json::extract_string(obj, "number") {
            if number == phone {
                if let Some(uuid) = json::extract_string(obj, "uuid") {
                    return Ok(uuid);
                }
            }
        }
    }

    Err("Bot UUID not found in identities".to_string())
}

/// Check if the envelope's mentions array references the bot.
/// Checks for any of the bot's identifiers (UUID, phone, username) in the mentions section.
fn has_bot_mention(envelope_json: &str, bot_id: &str) -> bool {
    let Some(mentions_idx) = envelope_json.find("\"mentions\"") else {
        return false;
    };
    let after_mentions = &envelope_json[mentions_idx..];
    let Some(end_idx) = after_mentions.find(']') else {
        return false;
    };
    let mentions_section = &after_mentions[..end_idx + 1];
    mentions_section.contains(bot_id)
}

/// Send a message with a base64 image attachment to a group.
pub fn send_image(
    host: &str,
    port: u16,
    phone: &str,
    group_id: &str,
    caption: &str,
    image_data: &[u8],
) -> Result<(), String> {
    use crate::base64;
    let b64 = base64::encode(image_data);
    let json_body = format!(
        r#"{{"message":"{}","number":"{}","recipients":["{}"],"base64_attachments":["data:image/png;base64,{}"]}}"#,
        json::escape(caption),
        json::escape(phone),
        json::escape(group_id),
        b64,
    );

    let (status, body) = http::http_post(host, port, "/v2/send", &json_body)?;

    if status != 201 && status != 200 {
        if status == 400 && body.contains("Unregistered user") {
            eprintln!("Warning: image sent but some recipients are unregistered");
            return Ok(());
        }
        return Err(format!("Send image failed (HTTP {}): {}", status, body));
    }

    Ok(())
}

/// Build the JSON body for a send request.
pub fn build_send_body(phone: &str, group_id: &str, message: &str) -> String {
    format!(
        r#"{{"message":"{}","number":"{}","recipients":["{}"],"text_mode":"styled"}}"#,
        json::escape(message),
        json::escape(phone),
        json::escape(group_id),
    )
}

/// Send a message to a specific group.
pub fn send_message(
    host: &str,
    port: u16,
    phone: &str,
    group_id: &str,
    message: &str,
) -> Result<(), String> {
    let json_body = build_send_body(phone, group_id, message);
    let (status, body) = http::http_post(host, port, "/v2/send", &json_body)?;

    // 200/201 = full success, 400 with "Unregistered user" = partial success (some members left Signal)
    if status != 201 && status != 200 {
        if status == 400 && body.contains("Unregistered user") {
            eprintln!("Warning: message sent but some recipients are unregistered");
            return Ok(());
        }
        return Err(format!("Send message failed (HTTP {}): {}", status, body));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_groups_realistic() {
        let body = r#"[
            {
                "name": "Family Chat",
                "id": "group.MkRpY0VRNWxPblNjNTE2VHRl",
                "internal_id": "2DicEQ5lOnSc516Tte0nVAd4",
                "members": ["+1234567890", "+0987654321"],
                "blocked": false,
                "admins": ["+1234567890"]
            },
            {
                "name": "Work Team",
                "id": "group.ckRzaEd4VmRzNnJaASAEsasa",
                "internal_id": "rGhGeVds6rZAIAQasa",
                "members": ["+1234567890"],
                "blocked": false,
                "admins": []
            }
        ]"#;

        let groups = parse_groups(body);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].name, "Family Chat");
        assert_eq!(groups[0].id, "group.MkRpY0VRNWxPblNjNTE2VHRl");
        assert_eq!(groups[1].name, "Work Team");
        assert_eq!(groups[1].id, "group.ckRzaEd4VmRzNnJaASAEsasa");
    }

    #[test]
    fn test_parse_groups_empty() {
        let groups = parse_groups("[]");
        assert_eq!(groups.len(), 0);
    }

    #[test]
    fn test_parse_messages_realistic() {
        let body = r#"[
            {
                "envelope": {
                    "source": "+1987654321",
                    "sourceNumber": "+1987654321",
                    "sourceDevice": 1,
                    "timestamp": 1612041718367,
                    "dataMessage": {
                        "timestamp": 1612041718367,
                        "message": "Hello there!",
                        "expiresInSeconds": 0,
                        "viewOnce": false,
                        "groupInfo": {
                            "groupId": "group.abc123",
                            "type": "DELIVER"
                        }
                    }
                }
            },
            {
                "envelope": {
                    "source": "+1555000111",
                    "timestamp": 1612041800000,
                    "dataMessage": {
                        "timestamp": 1612041800000,
                        "message": "Hi everyone!",
                        "groupInfo": {
                            "groupId": "group.abc123",
                            "type": "DELIVER"
                        }
                    }
                }
            }
        ]"#;

        let messages = parse_messages(body, "+0000000000");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].sender, "+1987654321");
        assert_eq!(messages[0].text, "Hello there!");
        assert_eq!(messages[0].timestamp, 1612041718367);
        assert_eq!(messages[0].group_id.as_deref(), Some("group.abc123"));
        assert_eq!(messages[1].sender, "+1555000111");
        assert_eq!(messages[1].text, "Hi everyone!");
    }

    #[test]
    fn test_parse_messages_no_group() {
        let body = r#"[
            {
                "envelope": {
                    "source": "+1987654321",
                    "timestamp": 1612041718367,
                    "dataMessage": {
                        "timestamp": 1612041718367,
                        "message": "Direct message"
                    }
                }
            }
        ]"#;

        let messages = parse_messages(body, "+0000000000");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].text, "Direct message");
        assert_eq!(messages[0].group_id, None);
    }

    #[test]
    fn test_parse_messages_empty_text_skipped() {
        let body = r#"[
            {
                "envelope": {
                    "source": "+1987654321",
                    "timestamp": 1612041718367,
                    "dataMessage": {
                        "timestamp": 1612041718367,
                        "message": ""
                    }
                }
            }
        ]"#;

        let messages = parse_messages(body, "+0000000000");
        assert_eq!(messages.len(), 0);
    }

    #[test]
    fn test_parse_messages_no_message_field_skipped() {
        // Envelopes without dataMessage.message (e.g., receipt messages) should be skipped
        let body = r#"[
            {
                "envelope": {
                    "source": "+1987654321",
                    "timestamp": 1612041718367,
                    "receiptMessage": {
                        "when": 1612041718367,
                        "isDelivery": true
                    }
                }
            }
        ]"#;

        let messages = parse_messages(body, "+0000000000");
        assert_eq!(messages.len(), 0);
    }

    #[test]
    fn test_parse_messages_empty_array() {
        let messages = parse_messages("[]", "+0000000000");
        assert_eq!(messages.len(), 0);
    }

    #[test]
    fn test_parse_messages_uses_source_number_fallback() {
        let body = r#"[
            {
                "envelope": {
                    "sourceNumber": "+1999888777",
                    "timestamp": 1612041718367,
                    "dataMessage": {
                        "timestamp": 1612041718367,
                        "message": "Fallback sender"
                    }
                }
            }
        ]"#;

        let messages = parse_messages(body, "+0000000000");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].sender, "+1999888777");
    }

    #[test]
    fn test_parse_messages_with_source_name() {
        let body = r#"[
            {
                "envelope": {
                    "source": "+1987654321",
                    "sourceName": "Alice",
                    "timestamp": 1612041718367,
                    "dataMessage": {
                        "timestamp": 1612041718367,
                        "message": "Hello!",
                        "groupInfo": {
                            "groupId": "group.abc123",
                            "type": "DELIVER"
                        }
                    }
                }
            },
            {
                "envelope": {
                    "source": "+1555000111",
                    "timestamp": 1612041800000,
                    "dataMessage": {
                        "timestamp": 1612041800000,
                        "message": "Hi!",
                        "groupInfo": {
                            "groupId": "group.abc123",
                            "type": "DELIVER"
                        }
                    }
                }
            }
        ]"#;

        let messages = parse_messages(body, "+0000000000");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].sender_name.as_deref(), Some("Alice"));
        assert_eq!(messages[1].sender_name, None);
    }

    #[test]
    fn test_parse_messages_detects_bot_mention_by_uuid() {
        let body = r#"[
            {
                "envelope": {
                    "source": "+1987654321",
                    "sourceName": "Alice",
                    "timestamp": 1612041718367,
                    "dataMessage": {
                        "timestamp": 1612041718367,
                        "message": "\uFFFC summarize please",
                        "mentions": [{"start": 0, "length": 1, "uuid": "00000000-0000-0000-0000-000000000099"}],
                        "groupInfo": {"groupId": "group.abc123", "type": "DELIVER"}
                    }
                }
            }
        ]"#;

        let messages = parse_messages(body, "00000000-0000-0000-0000-000000000099");
        assert_eq!(messages.len(), 1);
        assert!(messages[0].mentions_bot);
    }

    #[test]
    fn test_parse_messages_detects_bot_mention_by_phone() {
        let body = r#"[
            {
                "envelope": {
                    "source": "+1987654321",
                    "timestamp": 1612041718367,
                    "dataMessage": {
                        "timestamp": 1612041718367,
                        "message": "\uFFFC hi",
                        "mentions": [{"start": 0, "length": 1, "uuid": "some-uuid", "number": "+1111111111"}],
                        "groupInfo": {"groupId": "group.abc123", "type": "DELIVER"}
                    }
                }
            }
        ]"#;

        let messages = parse_messages(body, "+1111111111");
        assert_eq!(messages.len(), 1);
        assert!(messages[0].mentions_bot);
    }

    #[test]
    fn test_parse_messages_no_mention_no_trigger() {
        let body = r#"[
            {
                "envelope": {
                    "source": "+1987654321",
                    "timestamp": 1612041718367,
                    "dataMessage": {
                        "timestamp": 1612041718367,
                        "message": "summarize please",
                        "groupInfo": {"groupId": "group.abc123", "type": "DELIVER"}
                    }
                }
            }
        ]"#;

        let messages = parse_messages(body, "00000000-0000-0000-0000-000000000099");
        assert_eq!(messages.len(), 1);
        assert!(!messages[0].mentions_bot);
    }

    #[test]
    fn test_parse_messages_mention_other_user_no_trigger() {
        let body = r#"[
            {
                "envelope": {
                    "source": "+1987654321",
                    "timestamp": 1612041718367,
                    "dataMessage": {
                        "timestamp": 1612041718367,
                        "message": "\uFFFC what do you think?",
                        "mentions": [{"start": 0, "length": 1, "uuid": "other-uuid-here", "number": "+2222222222"}],
                        "groupInfo": {"groupId": "group.abc123", "type": "DELIVER"}
                    }
                }
            }
        ]"#;

        let messages = parse_messages(body, "00000000-0000-0000-0000-000000000099");
        assert_eq!(messages.len(), 1);
        assert!(!messages[0].mentions_bot);
    }

    #[test]
    fn test_build_send_body_basic() {
        let body = build_send_body("+1234567890", "group.abc123", "Hello world");
        assert!(body.contains(r#""message":"Hello world""#));
        assert!(body.contains(r#""number":"+1234567890""#));
        assert!(body.contains(r#""recipients":["group.abc123"]"#));
        assert!(body.contains(r#""text_mode":"styled""#));
    }

    #[test]
    fn test_build_send_body_escapes_special_chars() {
        let body = build_send_body("+1234567890", "group.abc", "Line1\nLine2 \"quoted\"");
        assert!(body.contains(r#"Line1\nLine2 \"quoted\""#));
    }

    #[test]
    fn test_build_send_body_with_markdown() {
        let body = build_send_body("+1234567890", "group.abc", "**Bold** and *italic*");
        assert!(body.contains(r#"**Bold** and *italic*"#));
    }
}
