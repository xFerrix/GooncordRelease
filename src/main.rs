#![windows_subsystem = "windows"]
use bcrypt::{hash, verify, DEFAULT_COST};
use chrono::{DateTime, Utc};
use rand::Rng;
use reqwest::blocking::Client;
use reqwest::header;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use uuid::Uuid;
use web_view::*;

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "type")]
enum WebMessage {
    Message { content: String },
    BanStatus,
    Register { username: String, password: String },
    Login { username: String, password: String },
    CheckAuth,
    RequestMessages,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ReceivedMessage {
    pub content: String,
    pub username: String,
    pub created_at: String,
    pub user_id: String,
    pub skip_polling: bool,
}

struct AppState {
    is_banned: bool,
    ban_reason: Option<String>,
    ban_expires: Option<DateTime<Utc>>,
    last_ban_check: Instant,
    supabase_url: String,
    supabase_key: String,
    session_token: Option<String>,
    client: Client,
    logged_in: bool,
    current_user_id: Option<Uuid>,
    current_username: Option<String>,
    message_sender: mpsc::Sender<ReceivedMessage>,
}

impl AppState {
    fn new() -> Self {
        let supabase_url = "Not giving you my url either".to_string();
        let supabase_key = "I am not giving you my key".to_string();

        let (tx, _) = mpsc::channel();

        AppState {
            is_banned: false,
            ban_reason: None,
            ban_expires: None,
            last_ban_check: Instant::now(),
            supabase_url,
            supabase_key,
            session_token: None,
            client: Client::new(),
            logged_in: false,
            current_user_id: None,
            current_username: None,
            message_sender: tx,
        }
    }

    fn start_message_cleanup(&self) {
        let state = self.clone();
        thread::spawn(move || {
            loop {
                thread::sleep(Duration::from_secs(120)); // 2 minutes

                if let Err(e) = state.cleanup_messages() {
                    eprintln!("Failed to cleanup messages: {}", e);
                }
            }
        });
    }

    fn cleanup_messages(&self) -> Result<(), String> {
        // First get all message IDs
        let url = format!(
            "{}/rest/v1/messages?select=id",
            self.supabase_url
        );

        let response = self
            .client
            .get(&url)
            .header(
                header::AUTHORIZATION,
                format!("Bearer {}", self.supabase_key),
            )
            .header("apikey", &self.supabase_key)
            .send()
            .map_err(|e| format!("Failed to fetch messages: {}", e))?;

        if !response.status().is_success() {
            let error = response.text().map_err(|e| e.to_string())?;
            return Err(format!("API error: {}", error));
        }

        #[derive(Deserialize)]
        struct MessageId {
            id: String,
        }

        let message_ids: Vec<MessageId> = response
            .json()
            .map_err(|e| format!("Failed to parse message IDs: {}", e))?;

        // Delete messages in batches to avoid overloading the server
        for chunk in message_ids.chunks(100) {
            let ids: Vec<&str> = chunk.iter().map(|m| m.id.as_str()).collect();
            let in_clause = ids.join(",");

            let delete_url = format!(
                "{}/rest/v1/messages?id=in.({})",
                self.supabase_url, in_clause
            );

            let delete_response = self
                .client
                .delete(&delete_url)
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", self.supabase_key),
                )
                .header("apikey", &self.supabase_key)
                .send()
                .map_err(|e| format!("Failed to delete messages: {}", e))?;

            if !delete_response.status().is_success() {
                let error = delete_response.text().map_err(|e| e.to_string())?;
                return Err(format!("API error during deletion: {}", error));
            }

            // Small delay between batches to avoid rate limiting
            thread::sleep(Duration::from_millis(200));
        }

        Ok(())
    }

    fn start_message_polling(&self) {
        let state = self.clone();
        thread::spawn(move || {
            let mut last_message_time: Option<DateTime<Utc>> = None;

            loop {
                thread::sleep(Duration::from_millis(10));

                if let Ok(messages) = state.get_messages() {
                    for msg in messages {
                        if let Ok(created_at) = DateTime::parse_from_rfc3339(&msg.created_at) {
                            let created_at = created_at.with_timezone(&Utc);

                            if last_message_time.map_or(true, |last| created_at > last) {
                                if let Err(e) = state.message_sender.send(msg.clone()) {
                                    eprintln!("Failed to send message: {}", e);
                                }
                                last_message_time = Some(created_at);
                            }
                        }
                    }
                }
            }
        });
    }

    fn get_messages(&self) -> Result<Vec<ReceivedMessage>, String> {
        let url = format!(
            "{}/rest/v1/messages?select=content,created_at,user_id,users(username)&order=created_at.desc&limit=20",
            self.supabase_url
        );

        let response = self
            .client
            .get(&url)
            .header(
                header::AUTHORIZATION,
                format!("Bearer {}", self.supabase_key),
            )
            .header("apikey", &self.supabase_key)
            .send()
            .map_err(|e| format!("Failed to fetch messages: {}", e))?;

        if !response.status().is_success() {
            let error = response.text().map_err(|e| e.to_string())?;
            return Err(format!("API error: {}", error));
        }

        #[derive(Deserialize)]
        struct MessageWithUser {
            content: String,
            created_at: String,
            user_id: String,
            users: User,
        }

        #[derive(Deserialize)]
        struct User {
            username: String,
        }

        let messages_with_users: Vec<MessageWithUser> = response
            .json()
            .map_err(|e| format!("Failed to parse messages: {}", e))?;

        let mut messages = messages_with_users
            .into_iter()
            .map(|msg| {
                let created_at = DateTime::parse_from_rfc3339(&msg.created_at)
                    .map_err(|e| format!("Invalid timestamp: {}", e))?
                    .with_timezone(&Utc)
                    .to_rfc3339();

                Ok(ReceivedMessage {
                    content: msg.content,
                    username: msg.users.username,
                    created_at,
                    user_id: msg.user_id,
                    skip_polling: false,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        messages.reverse();
        Ok(messages)
    }

    fn generate_session_token() -> String {
        "dummy_token".to_string()
    }

    fn register_user(&mut self, username: &str, password: &str) -> Result<String, String> {
        let hashed_password =
            hash(password, DEFAULT_COST).map_err(|e| format!("Password hashing failed: {}", e))?;

        let check_url = format!(
            "{}/rest/v1/users?username=eq.{}",
            self.supabase_url, username
        );
        let response = self
            .client
            .get(&check_url)
            .header(
                header::AUTHORIZATION,
                format!("Bearer {}", self.supabase_key),
            )
            .header("apikey", &self.supabase_key)
            .send()
            .map_err(|e| format!("Username check failed: {}", e))?;

        if response.status().is_success() {
            let users: Vec<serde_json::Value> = response
                .json()
                .map_err(|e| format!("Failed to parse response: {}", e))?;

            if !users.is_empty() {
                return Err("Username already exists".to_string());
            }
        }

        let user_id = Uuid::new_v4();
        let create_url = format!("{}/rest/v1/users", self.supabase_url);
        let response = self
            .client
            .post(&create_url)
            .header(
                header::AUTHORIZATION,
                format!("Bearer {}", self.supabase_key),
            )
            .header("apikey", &self.supabase_key)
            .header("Content-Type", "application/json")
            .header("Prefer", "return=minimal")
            .json(&json!({
                "id": user_id,
                "username": username,
                "password_hash": hashed_password,
                "created_at": Utc::now().to_rfc3339()
            }))
            .send()
            .map_err(|e| format!("User creation failed: {}", e))?;

        if !response.status().is_success() {
            let error = response.text().map_err(|e| e.to_string())?;
            return Err(format!("User creation failed: {}", error));
        }

        self.create_session(user_id, username)
    }

    fn create_session(&mut self, user_id: Uuid, username: &str) -> Result<String, String> {
        let session_token = Self::generate_session_token();
        let expires_at = Utc::now() + chrono::Duration::days(30);

        let session_url = format!("{}/rest/v1/sessions", self.supabase_url);
        let response = self
            .client
            .post(&session_url)
            .header(
                header::AUTHORIZATION,
                format!("Bearer {}", self.supabase_key),
            )
            .header("apikey", &self.supabase_key)
            .header("Content-Type", "application/json")
            .header("Prefer", "return=minimal")
            .json(&json!({
                "user_id": user_id,
                "token": session_token,
                "expires_at": expires_at.to_rfc3339()
            }))
            .send()
            .map_err(|e| format!("Session creation failed: {}", e))?;

        if !response.status().is_success() {
            let error = response.text().map_err(|e| e.to_string())?;
            return Err(format!("Session creation failed: {}", error));
        }

        self.session_token = Some(session_token.clone());
        self.logged_in = true;
        self.current_user_id = Some(user_id);
        self.current_username = Some(username.to_string());

        Ok("Registration successful! You are now logged in.".to_string())
    }

    fn login_user(&mut self, username: &str, password: &str) -> Result<String, String> {
        let url = format!(
            "{}/rest/v1/users?username=eq.{}",
            self.supabase_url, username
        );
        let response = self
            .client
            .get(&url)
            .header(
                header::AUTHORIZATION,
                format!("Bearer {}", self.supabase_key),
            )
            .header("apikey", &self.supabase_key)
            .send()
            .map_err(|e| format!("Login failed: {}", e))?;

        if !response.status().is_success() {
            return Err("User not found".to_string());
        }

        let users: Vec<serde_json::Value> = response
            .json()
            .map_err(|e| format!("Failed to parse response: {}", e))?;

        let user = users.first().ok_or("User not found")?;
        let user_id = Uuid::parse_str(user["id"].as_str().ok_or("Invalid user ID")?)
            .map_err(|e| format!("Invalid user ID: {}", e))?;

        let stored_hash = user["password_hash"]
            .as_str()
            .ok_or("Invalid password hash")?;

        if !verify(password, stored_hash)
            .map_err(|e| format!("Password verification failed: {}", e))?
        {
            return Err("Invalid password".to_string());
        }

        self.create_session(user_id, username)?;
        self.check_ban_status()?;
        Ok("Login successful!".to_string())
    }

    fn check_ban_status(&mut self) -> Result<(), String> {
        let Some(user_id) = self.current_user_id else {
            return Err("User not logged in".to_string());
        };

        let url = format!(
            "{}/rest/v1/bans?user_id=eq.{}&select=reason,expires_at,is_active",
            self.supabase_url, user_id
        );

        let response = self
            .client
            .get(&url)
            .header(
                header::AUTHORIZATION,
                format!("Bearer {}", self.supabase_key),
            )
            .header("apikey", &self.supabase_key)
            .send()
            .map_err(|e| e.to_string())?;

        if response.status().is_success() {
            let bans: Vec<serde_json::Value> = response.json().map_err(|e| e.to_string())?;

            if let Some(ban) = bans.first() {
                if ban["is_active"] == true {
                    if let (Some(reason), Some(expires_at)) =
                        (ban["reason"].as_str(), ban["expires_at"].as_str())
                    {
                        let expires = DateTime::parse_from_rfc3339(expires_at)
                            .map_err(|e| e.to_string())?
                            .with_timezone(&Utc);

                        if expires > Utc::now() {
                            self.is_banned = true;
                            self.ban_reason = Some(reason.to_string());
                            self.ban_expires = Some(expires);
                        } else {
                            self.is_banned = false;
                            self.ban_reason = None;
                            self.ban_expires = None;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn send_message(&self, content: &str) -> Result<ReceivedMessage, String> {
        let Some(user_id) = self.current_user_id else {
            return Err("User not identified".to_string());
        };

        // First check if we need to delete old messages
        let count_url = format!(
            "{}/rest/v1/messages?select=id",
            self.supabase_url
        );

        let count_response = self
            .client
            .get(&count_url)
            .header(
                header::AUTHORIZATION,
                format!("Bearer {}", self.supabase_key),
            )
            .header("apikey", &self.supabase_key)
            .header("Content-Range", "0-9")
            .send()
            .map_err(|e| e.to_string())?;

        if count_response.status().is_success() {
            if let Some(content_range) = count_response.headers().get(header::CONTENT_RANGE) {
                if let Ok(content_range_str) = content_range.to_str() {
                    if let Some(total_str) = content_range_str.split('/').nth(1) {
                        if let Ok(total) = total_str.parse::<usize>() {
                            if total >= 20 {
                                // Get the oldest message ID
                                let oldest_url = format!(
                                    "{}/rest/v1/messages?select=id&order=created_at.asc&limit=1",
                                    self.supabase_url
                                );

                                let oldest_response = self
                                    .client
                                    .get(&oldest_url)
                                    .header(
                                        header::AUTHORIZATION,
                                        format!("Bearer {}", self.supabase_key),
                                    )
                                    .header("apikey", &self.supabase_key)
                                    .send()
                                    .map_err(|e| e.to_string())?;

                                if oldest_response.status().is_success() {
                                    let oldest_messages: Vec<serde_json::Value> = oldest_response
                                        .json()
                                        .map_err(|e| format!("Failed to parse oldest message: {}", e))?;

                                    if let Some(oldest_message) = oldest_messages.first() {
                                        if let Some(oldest_id) = oldest_message["id"].as_str() {
                                            // Delete the oldest message
                                            let delete_url = format!(
                                                "{}/rest/v1/messages?id=eq.{}",
                                                self.supabase_url, oldest_id
                                            );

                                            let _ = self
                                                .client
                                                .delete(&delete_url)
                                                .header(
                                                    header::AUTHORIZATION,
                                                    format!("Bearer {}", self.supabase_key),
                                                )
                                                .header("apikey", &self.supabase_key)
                                                .send()
                                                .map_err(|e| e.to_string())?;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Now send the new message
        let url = format!("{}/rest/v1/messages", self.supabase_url);

        let response = self
            .client
            .post(&url)
            .header(
                header::AUTHORIZATION,
                format!("Bearer {}", self.supabase_key),
            )
            .header("apikey", &self.supabase_key)
            .header("Content-Type", "application/json")
            .header("Prefer", "return=representation")
            .json(&json!({
                "content": content,
                "user_id": user_id
            }))
            .send()
            .map_err(|e| e.to_string())?;

        if response.status().is_success() {
            let mut msg: ReceivedMessage = response
                .json()
                .map_err(|e| format!("Failed to parse response: {}", e))?;

            msg.username = self.current_username.clone().unwrap_or("You".to_string());
            msg.skip_polling = true;

            Ok(msg)
        } else {
            let error = response.text().map_err(|e| e.to_string())?;
            Err(error)
        }
    }

    fn check_random_ban(&mut self) -> Result<bool, String> {
        if self.is_banned || !self.logged_in {
            return Ok(false);
        }

        if rand::thread_rng().gen_range(0.0..1.0) < 0.05 {
            let ban_reasons = [
                "illegal emoji usage",
                "excessive happiness",
                "breathing too loudly",
                "suspicious typing patterns",
                "the Random Ban God's will",
                "using Comic Sans unironically",
                "sending messages too fast",
                "sending messages too slow",
                "liking pineapple on pizza",
                "disliking pineapple on pizza",
                "existing",
                "spelling mistakes",
                "capitalizing every word",
                "using too many exclamation marks!!!",
                "being too polite",
                "being too rude",
                "suspicious silence",
                "laughing too much",
                "not laughing enough",
                "incorrect opinion detected",
                "overusing GIFs",
                "speaking forbidden languages",
                "sending cursed images",
                "being a bot (maybe)",
                "having a suspiciously cool username",
                "having no profile picture",
                "having too many profile pictures",
                "sending memes at 3 AM",
                "excessive lurking",
                "breathing in Morse code",
                "using tabs instead of spaces",
                "using spaces instead of tabs",
                "being too smart",
                "being too dumb",
                "using forbidden words",
                "using forbidden thoughts",
                "being suspiciously normal",
                "changing nicknames too often",
                "having a lucky day",
                "having an unlucky day",
                "responding to bots",
                "arguing with moderators",
                "being too relatable",
                "winning too many arguments",
                "losing too many arguments",
                "complaining about random bans",
                "random quantum fluctuations",
                "karma imbalance detected",
                "the server hamster tripped",
                "too much drip",
                "not enough drip",
                "interdimensional travel violations",
            ];

            let reason = ban_reasons[rand::thread_rng().gen_range(0..ban_reasons.len())];
            let duration_secs = rand::thread_rng().gen_range(10..30);
            let expires_at = Utc::now() + chrono::Duration::seconds(duration_secs);

            let url = format!("{}/rest/v1/bans", self.supabase_url);

            let response = self
                .client
                .post(&url)
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", self.supabase_key),
                )
                .header("apikey", &self.supabase_key)
                .json(&json!({
                    "user_id": self.current_user_id,
                    "reason": reason,
                    "expires_at": expires_at.to_rfc3339(),
                    "is_active": true
                }))
                .send()
                .map_err(|e| e.to_string())?;

            if response.status().is_success() {
                self.is_banned = true;
                self.ban_reason = Some(reason.to_string());
                self.ban_expires = Some(expires_at);
                Ok(true)
            } else {
                Err("Failed to create ban".to_string())
            }
        } else {
            Ok(false)
        }
    }
}

impl Clone for AppState {
    fn clone(&self) -> Self {
        AppState {
            is_banned: self.is_banned,
            ban_reason: self.ban_reason.clone(),
            ban_expires: self.ban_expires,
            last_ban_check: self.last_ban_check,
            supabase_url: self.supabase_url.clone(),
            supabase_key: self.supabase_key.clone(),
            session_token: self.session_token.clone(),
            client: Client::new(),
            logged_in: self.logged_in,
            current_user_id: self.current_user_id,
            current_username: self.current_username.clone(),
            message_sender: self.message_sender.clone(),
        }
    }
}

fn main() -> WVResult {
    let mut state = AppState::new();
    let (tx, rx) = mpsc::channel();
    state.message_sender = tx;
    state.start_message_polling();
    state.start_message_cleanup();
    web_view::builder()
        .title("Gooncord")
        .content(Content::Html(include_str!("index.html")))
        .size(1200, 800)
        .resizable(true)
        .debug(true)
        .user_data(())
        .invoke_handler(move |webview, arg| {
            if state.is_banned {
                if let Some(expires) = state.ban_expires {
                    let remaining = (expires - Utc::now()).num_seconds();
                    if remaining > 0 {
                        let reason = state.ban_reason.as_deref().unwrap_or("no reason");
                        webview.eval(&format!(
                            "disableInput({}); showBan('{}');",
                            remaining,
                            reason.replace("'", "\\'")
                        ))?;
                        return Ok(());
                    } else {
                        state.is_banned = false;
                        state.ban_reason = None;
                        state.ban_expires = None;
                        webview.eval("enableInput();")?;
                    }
                }
            }

            while let Ok(msg) = rx.try_recv() {
                if !msg.skip_polling {
                    let avatar = msg.username.chars().next().unwrap_or('?').to_string();
                    webview.eval(&format!(
                        "addMessage({}, {}, {}, {}, false);",
                        escape_js_string(&msg.username),
                        escape_js_string(&avatar),
                        escape_js_string(&msg.content),
                        escape_js_string(&msg.created_at)
                    ))?;
                }
            }

            fn escape_js_string(s: &str) -> String {
                let mut escaped = String::with_capacity(s.len());
                for c in s.chars() {
                    match c {
                        '\'' => escaped.push_str("\\'"),
                        '\"' => escaped.push_str("\\\""),
                        '\\' => escaped.push_str("\\\\"),
                        '\n' => escaped.push_str("\\n"),
                        '\r' => escaped.push_str("\\r"),
                        '\t' => escaped.push_str("\\t"),
                        _ => escaped.push(c),
                    }
                }
                format!("'{}'", escaped)
            }

            match serde_json::from_str::<WebMessage>(arg) {
                Ok(WebMessage::Message { content }) => {
                    if !state.logged_in {
                        webview.eval("addSystemMessage('Please login first!');")?;
                        return Ok(());
                    }

                    if state.is_banned {
                        if let Some(expires) = state.ban_expires {
                            let remaining = (expires - Utc::now()).num_seconds();
                            if remaining > 0 {
                                let reason = state.ban_reason.as_deref().unwrap_or("no reason");
                                webview.eval(&format!(
                                    "disableInput({}); showBan('{}');",
                                    remaining,
                                    reason.replace("'", "\\'")
                                ))?;
                                return Ok(());
                            } else {
                                state.is_banned = false;
                                state.ban_reason = None;
                                state.ban_expires = None;
                                webview.eval("enableInput();")?;
                            }
                        }
                    }

                    match state.send_message(&content) {
                        Ok(msg) => {
                            let _avatar = msg.username.chars().next().unwrap_or('?').to_string();
                            let _ = state.message_sender.send(msg);
                        }
                        Err(_e) => {}
                    }
                    Ok(())
                }
                Ok(WebMessage::BanStatus) => {
                    if state.logged_in && !state.is_banned {
                        if state.last_ban_check.elapsed() >= Duration::from_secs(1) {
                            state.last_ban_check = Instant::now();
                            if let Ok(true) = state.check_random_ban() {
                                if let (Some(reason), Some(expires)) =
                                    (&state.ban_reason, state.ban_expires)
                                {
                                    let remaining = (expires - Utc::now()).num_seconds();
                                    let js_code = format!(
                                        "updateBanDisplay('{}', {});",
                                        reason.replace("'", "\\'"),
                                        remaining
                                    );
                                    if let Err(e) = webview.eval(&js_code) {
                                        eprintln!("Failed to execute JS: {}", e);
                                    }
                                }
                            }
                        }
                    }
                    else if state.is_banned {
                        if let Some(expires) = state.ban_expires {
                            let remaining = (expires - Utc::now()).num_seconds();
                            if remaining > 0 {
                                let _reason = state.ban_reason.as_deref().unwrap_or("no reason");
                                let js_code = format!("updateBanTimer({});", remaining);
                                webview.eval(&js_code)?;
                            } else {
                                state.is_banned = false;
                                state.ban_reason = None;
                                state.ban_expires = None;
                                webview.eval("clearBanDisplay();")?;
                            }
                        }
                    }
                    Ok(())
                }
                Ok(WebMessage::Register { username, password }) => {
                    match state.register_user(&username, &password) {
                        Ok(msg) => {
                            webview.eval("hideAuthForms();")?;
                            webview.eval(&format!(
                                "addSystemMessage('{}');",
                                msg.replace("'", "\\'")
                            ))?;
                            webview.eval("enableInput();")?;
                        }
                        Err(e) => webview.eval(&format!(
                            "addSystemMessage('Registration failed: {}');",
                            e.replace("'", "\\'")
                        ))?,
                    }
                    Ok(())
                }
                Ok(WebMessage::Login { username, password }) => {
                    match state.login_user(&username, &password) {
                        Ok(msg) => {
                            webview.eval("hideAuthForms();")?;
                            webview.eval(&format!(
                                "addSystemMessage('{}');",
                                msg.replace("'", "\\'")
                            ))?;
                            if !state.is_banned {
                                webview.eval("enableInput();")?;
                            }
                        }
                        Err(e) => webview.eval(&format!(
                            "addSystemMessage('Login failed: {}');",
                            e.replace("'", "\\'")
                        ))?,
                    }
                    Ok(())
                }
                Ok(WebMessage::CheckAuth) => {
                    if state.logged_in {
                        webview.eval("hideAuthForms();")?;
                        if !state.is_banned {
                            webview.eval("enableInput();")?;
                        }
                    } else {
                        webview.eval("showAuthForms();")?;
                    }
                    Ok(())
                }
                Ok(WebMessage::RequestMessages) => {
                    Ok(())
                }
                Err(e) => {
                    eprintln!("Failed to parse message: {}", e);
                    Ok(())
                }
            }
        })
        .run()
}