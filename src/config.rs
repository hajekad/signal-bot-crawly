use std::env;

pub struct Config {
    pub signal_api_host: String,
    pub signal_api_port: u16,
    pub webui_host: String,
    pub webui_port: u16,
    pub webui_api_key: String,
    pub model: String,
    pub signal_phone: String,
    pub schedule: Schedule,
    pub summary_prompt: String,
}

#[derive(Debug, Clone, Copy)]
pub enum Schedule {
    Daily,
    Weekly,
    Monthly,
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        let signal_url = env::var("SIGNAL_API_URL").unwrap_or_else(|_| "http://signal-api:8080".to_string());
        let webui_url = env::var("OPEN_WEBUI_URL").unwrap_or_else(|_| "http://open-webui:8080".to_string());

        let (signal_api_host, signal_api_port) = crate::http::parse_url(&signal_url)?;
        let (webui_host, webui_port) = crate::http::parse_url(&webui_url)?;

        let webui_api_key = env::var("OPEN_WEBUI_API_KEY")
            .map_err(|_| "OPEN_WEBUI_API_KEY environment variable is required".to_string())?;

        let signal_phone = env::var("SIGNAL_PHONE_NUMBER")
            .map_err(|_| "SIGNAL_PHONE_NUMBER environment variable is required".to_string())?;

        let schedule = match env::var("SCHEDULE").unwrap_or_else(|_| "weekly".to_string()).to_lowercase().as_str() {
            "daily" => Schedule::Daily,
            "weekly" => Schedule::Weekly,
            "monthly" => Schedule::Monthly,
            other => return Err(format!("Invalid SCHEDULE '{}': must be daily, weekly, or monthly", other)),
        };

        let summary_prompt = env::var("SUMMARY_PROMPT").unwrap_or_else(|_| {
            "You are a concise summarizer of group chat messages. \
             Given chat messages, produce a brief summary capturing key topics discussed, \
             decisions made, action items, and important context. Use bullet points."
                .to_string()
        });

        let model = env::var("OLLAMA_MODEL").unwrap_or_else(|_| "gpt-oss:20b".to_string());

        Ok(Config {
            signal_api_host,
            signal_api_port,
            webui_host,
            webui_port,
            webui_api_key,
            model,
            signal_phone,
            schedule,
            summary_prompt,
        })
    }
}
