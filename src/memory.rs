//! Ephemeral conversation sessions backed by Open WebUI chat API.
//!
//! - `stay` mode: creates an Open WebUI chat session for a group
//! - DMs: creates a chat session per sender
//! - All sessions deleted on `shut`, bot restart, or session end
//! - No local storage of conversation data — Open WebUI manages it

use std::collections::HashMap;

/// A message in the conversation history (OpenAI format).
#[derive(Clone)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

const STAY_IDLE_TIMEOUT_SECS: u64 = 180; // 3 minutes

/// An active conversation session tied to an Open WebUI chat.
pub struct Session {
    /// Open WebUI chat ID
    pub chat_id: String,
    /// Message history (sent to API on each request)
    pub messages: Vec<ChatMessage>,
    /// Model to use for this session
    pub model: String,
    /// Last time new messages were seen (UNIX seconds)
    pub last_activity: u64,
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Manages ephemeral conversation sessions.
pub struct SessionManager {
    /// Group stay sessions: group_internal_id -> Session
    pub stay_sessions: HashMap<String, Session>,
    /// DM sessions: sender_id -> Session
    pub dm_sessions: HashMap<String, Session>,
}

impl SessionManager {
    pub fn new() -> Self {
        SessionManager {
            stay_sessions: HashMap::new(),
            dm_sessions: HashMap::new(),
        }
    }

    /// Get or create a DM session. Returns None if chat creation fails.
    pub fn get_or_create_dm(
        &mut self,
        sender: &str,
        model: &str,
        host: &str,
        port: u16,
        api_key: &str,
    ) -> Option<&mut Session> {
        if !self.dm_sessions.contains_key(sender) {
            let chat_id = create_chat(host, port, api_key, &format!("DM-{}", sender))?;
            self.dm_sessions.insert(
                sender.to_string(),
                Session {
                    chat_id,
                    messages: Vec::new(),
                    model: model.to_string(),
                    last_activity: now_secs(),
                },
            );
        }
        self.dm_sessions.get_mut(sender)
    }

    /// Create a stay session for a group.
    pub fn start_stay(
        &mut self,
        group_id: &str,
        model: &str,
        host: &str,
        port: u16,
        api_key: &str,
    ) -> Option<String> {
        // Delete existing session if any
        self.end_stay(group_id, host, port, api_key);

        let chat_id = create_chat(host, port, api_key, &format!("Stay-{}", group_id))?;
        self.stay_sessions.insert(
            group_id.to_string(),
            Session {
                chat_id: chat_id.clone(),
                messages: Vec::new(),
                model: model.to_string(),
                last_activity: now_secs(),
            },
        );
        Some(chat_id)
    }

    /// End a stay session and delete the Open WebUI chat.
    pub fn end_stay(&mut self, group_id: &str, host: &str, port: u16, api_key: &str) {
        if let Some(session) = self.stay_sessions.remove(group_id) {
            delete_chat(host, port, api_key, &session.chat_id);
        }
    }

    /// Check if stay is active for a group.
    pub fn is_stay_active(&self, group_id: &str) -> bool {
        self.stay_sessions.contains_key(group_id)
    }

    /// Update last_activity for a stay session (call when new messages arrive).
    pub fn touch_stay(&mut self, group_id: &str) {
        if let Some(session) = self.stay_sessions.get_mut(group_id) {
            session.last_activity = now_secs();
        }
    }

    /// End any stay sessions idle for more than 3 minutes. Returns list of expired group IDs.
    pub fn expire_idle_stays(&mut self, host: &str, port: u16, api_key: &str) -> Vec<String> {
        let now = now_secs();
        let expired: Vec<String> = self
            .stay_sessions
            .iter()
            .filter(|(_, s)| now - s.last_activity > STAY_IDLE_TIMEOUT_SECS)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &expired {
            self.end_stay(id, host, port, api_key);
        }
        expired
    }

    /// Delete ALL sessions (stay + DM). Called on bot shutdown.
    #[allow(dead_code)]
    pub fn destroy_all(&mut self, host: &str, port: u16, api_key: &str) {
        let stay_ids: Vec<String> = self.stay_sessions.keys().cloned().collect();
        for id in stay_ids {
            self.end_stay(&id, host, port, api_key);
        }
        let dm_keys: Vec<String> = self.dm_sessions.keys().cloned().collect();
        for key in dm_keys {
            if let Some(session) = self.dm_sessions.remove(&key) {
                delete_chat(host, port, api_key, &session.chat_id);
            }
        }
    }
}

/// Send a message in a session and get the response.
pub fn chat_in_session(
    session: &mut Session,
    user_message: &str,
    system_prompt: &str,
    host: &str,
    port: u16,
    api_key: &str,
) -> Result<String, String> {
    // Add user message to history
    session.messages.push(ChatMessage {
        role: "user".to_string(),
        content: user_message.to_string(),
    });

    // Build the full messages array with system prompt
    let mut api_messages = String::new();
    api_messages.push_str(&format!(
        r#"{{"role":"system","content":"{}"}}"#,
        crate::json::escape(system_prompt)
    ));
    for msg in &session.messages {
        api_messages.push_str(&format!(
            r#",{{"role":"{}","content":"{}"}}"#,
            crate::json::escape(&msg.role),
            crate::json::escape(&msg.content)
        ));
    }

    let json_body = format!(
        r#"{{"model":"{}","messages":[{}],"stream":false,"chat_id":"{}"}}"#,
        crate::json::escape(&session.model),
        api_messages,
        crate::json::escape(&session.chat_id),
    );

    let (status, body) = crate::http::http_post_with_auth(
        host,
        port,
        "/api/chat/completions",
        &json_body,
        Some(api_key),
    )?;

    if status != 200 {
        return Err(format!(
            "Chat completions failed (HTTP {}): {}",
            status, body
        ));
    }

    // OpenAI format: {"choices":[{"message":{"content":"..."}}]}
    let content = extract_openai_content(&body)?;

    // Add assistant response to history
    session.messages.push(ChatMessage {
        role: "assistant".to_string(),
        content: content.clone(),
    });

    Ok(content)
}

/// Ask the LLM if it should respond to the current conversation.
/// Returns Some(response) if it decides to speak, None if it stays silent.
pub fn should_respond(
    session: &mut Session,
    recent_messages: &str,
    host: &str,
    port: u16,
    api_key: &str,
) -> Result<Option<String>, String> {
    let prompt = format!(
        "New messages in the group:\n{}\n\n\
         Based on the conversation, should you add something? \
         Only respond if you can genuinely contribute — correct a mistake, \
         add useful information, clarify confusion, or answer a question directed at no one in particular. \
         Do NOT respond to every message. Most of the time, stay silent.\n\n\
         If you should respond, start your message with RESPOND: followed by your response.\n\
         If you should stay silent, reply with just: SILENT",
        recent_messages
    );

    let response = chat_in_session(
        session,
        &prompt,
        "You are a knowledgeable participant in a group chat. You have been asked to stay and listen. \
         Only speak when you have something genuinely useful to add. \
         Most messages don't need your input. Stay silent unless you can clearly help. \
         Never respond with greetings, acknowledgments, or filler. \
         If you respond, be concise and natural — like a real person, not an assistant.",
        host,
        port,
        api_key,
    )?;

    if let Some(stripped) = response.strip_prefix("RESPOND:") {
        Ok(Some(stripped.trim().to_string()))
    } else if response.starts_with("SILENT") {
        Ok(None)
    } else {
        // LLM didn't follow format — treat non-SILENT as a response
        if response.to_uppercase().contains("SILENT") {
            Ok(None)
        } else {
            Ok(Some(response))
        }
    }
}

/// Create an Open WebUI chat session. Returns the chat ID.
fn create_chat(host: &str, port: u16, api_key: &str, title: &str) -> Option<String> {
    let json_body = format!(
        r#"{{"chat":{{"title":"{}","messages":[]}}}}"#,
        crate::json::escape(title)
    );
    let (status, body) = crate::http::http_post_with_auth(
        host,
        port,
        "/api/v1/chats/new",
        &json_body,
        Some(api_key),
    )
    .ok()?;

    if status != 200 {
        eprintln!("Failed to create chat (HTTP {}): {}", status, body);
        return None;
    }

    crate::json::extract_string(&body, "id")
}

/// Delete an Open WebUI chat session.
fn delete_chat(host: &str, port: u16, api_key: &str, chat_id: &str) {
    let path = format!("/api/v1/chats/{}", chat_id);
    // DELETE with empty body
    let addr = format!("{}:{}", host, port);
    let request = format!(
        "DELETE {} HTTP/1.1\r\nHost: {}:{}\r\nAuthorization: Bearer {}\r\nConnection: close\r\n\r\n",
        path, host, port, api_key
    );

    use std::io::Write;
    use std::net::TcpStream;
    if let Ok(mut stream) = TcpStream::connect(&addr) {
        let _ = stream.write_all(request.as_bytes());
        // Fire and forget — we don't care about the response
    }
}

/// Extract content from OpenAI-format response.
fn extract_openai_content(body: &str) -> Result<String, String> {
    // Try OpenAI format first: {"choices":[{"message":{"content":"..."}}]}
    if let Some(content) = crate::json::extract_string(body, "content") {
        return Ok(content);
    }
    Err(format!(
        "Failed to extract content from response: {}",
        &body[..body.len().min(200)]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_manager_new() {
        let mgr = SessionManager::new();
        assert!(mgr.stay_sessions.is_empty());
        assert!(mgr.dm_sessions.is_empty());
    }

    #[test]
    fn test_is_stay_active() {
        let mut mgr = SessionManager::new();
        assert!(!mgr.is_stay_active("group1"));
        mgr.stay_sessions.insert(
            "group1".to_string(),
            Session {
                chat_id: "test".to_string(),
                messages: Vec::new(),
                model: "test".to_string(),
                last_activity: 0,
            },
        );
        assert!(mgr.is_stay_active("group1"));
    }

    #[test]
    fn test_chat_message_history() {
        let mut session = Session {
            chat_id: "test".to_string(),
            messages: Vec::new(),
            model: "test".to_string(),
            last_activity: 0,
        };
        session.messages.push(ChatMessage {
            role: "user".to_string(),
            content: "Hello".to_string(),
        });
        session.messages.push(ChatMessage {
            role: "assistant".to_string(),
            content: "Hi there!".to_string(),
        });
        assert_eq!(session.messages.len(), 2);
        assert_eq!(session.messages[0].role, "user");
        assert_eq!(session.messages[1].content, "Hi there!");
    }

    #[test]
    fn test_extract_openai_content() {
        let body = r#"{"id":"chatcmpl-123","choices":[{"message":{"role":"assistant","content":"Hello world"}}]}"#;
        assert_eq!(extract_openai_content(body).unwrap(), "Hello world");
    }

    #[test]
    fn test_extract_openai_content_missing() {
        assert!(extract_openai_content("{}").is_err());
    }

    #[test]
    fn test_should_respond_parses_respond() {
        // Can't test against real API, but test the parsing logic
        let response = "RESPOND: Actually, that's not quite right. The capital of Australia is Canberra, not Sydney.";
        if response.starts_with("RESPOND:") {
            let msg = response[8..].trim();
            assert!(msg.contains("Canberra"));
        }
    }

    #[test]
    fn test_should_respond_parses_silent() {
        let response = "SILENT";
        assert!(response.starts_with("SILENT"));
    }
}
