use crate::http;
use crate::json;

/// Send a chat completion request to Ollama and return the assistant's response.
pub fn summarize(
    host: &str,
    port: u16,
    model: &str,
    system_prompt: &str,
    user_message: &str,
) -> Result<String, String> {
    let json_body = build_chat_body(model, system_prompt, user_message);
    let (status, body) = http::http_post(host, port, "/api/chat", &json_body)?;

    if status != 200 {
        return Err(format!("Ollama chat failed (HTTP {}): {}", status, body));
    }

    parse_chat_response(&body)
}

/// Build the Ollama chat request JSON body.
pub fn build_chat_body(model: &str, system_prompt: &str, user_message: &str) -> String {
    format!(
        r#"{{"model":"{}","messages":[{{"role":"system","content":"{}"}},{{"role":"user","content":"{}"}}],"stream":false,"options":{{"temperature":0.3,"num_ctx":8192}}}}"#,
        json::escape(model),
        json::escape(system_prompt),
        json::escape(user_message),
    )
}

/// Parse the assistant content from an Ollama chat response.
pub fn parse_chat_response(body: &str) -> Result<String, String> {
    json::extract_string(body, "content")
        .ok_or_else(|| format!("Failed to extract content from Ollama response: {}", body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_chat_response_realistic() {
        let body = r#"{
            "model": "llama3.2",
            "message": {
                "role": "assistant",
                "content": "Here is the summary:\n\u2022 Team decided on PostgreSQL\n\u2022 Deadline is Friday"
            },
            "done": true,
            "done_reason": "stop",
            "total_duration": 2145000000,
            "eval_count": 67,
            "eval_duration": 1850000000
        }"#;

        let content = parse_chat_response(body).unwrap();
        assert!(content.starts_with("Here is the summary:"));
        assert!(content.contains("PostgreSQL"));
        assert!(content.contains("Friday"));
    }

    #[test]
    fn test_parse_chat_response_no_content() {
        let body = r#"{"model": "llama3.2", "done": false}"#;
        assert!(parse_chat_response(body).is_err());
    }

    #[test]
    fn test_build_chat_body_structure() {
        let body = build_chat_body("llama3.2", "You are helpful.", "Summarize this.");
        assert!(body.contains(r#""model":"llama3.2""#));
        assert!(body.contains(r#""role":"system""#));
        assert!(body.contains(r#""content":"You are helpful.""#));
        assert!(body.contains(r#""role":"user""#));
        assert!(body.contains(r#""content":"Summarize this.""#));
        assert!(body.contains(r#""stream":false"#));
        assert!(body.contains(r#""num_ctx":8192"#));
    }

    #[test]
    fn test_build_chat_body_escapes_newlines() {
        let body = build_chat_body("llama3.2", "Be concise.", "[Alice]: Hi\n[Bob]: Hello");
        assert!(body.contains(r#"[Alice]: Hi\n[Bob]: Hello"#));
    }

    #[test]
    fn test_build_chat_body_escapes_quotes() {
        let body = build_chat_body("llama3.2", "Be concise.", r#"She said "hello""#);
        assert!(body.contains(r#"She said \"hello\""#));
    }

    #[test]
    fn test_parse_chat_response_with_special_chars() {
        let body = r#"{"message":{"role":"assistant","content":"Summary:\n- Item \"one\"\n- Item two"},"done":true}"#;
        let content = parse_chat_response(body).unwrap();
        assert_eq!(content, "Summary:\n- Item \"one\"\n- Item two");
    }
}
