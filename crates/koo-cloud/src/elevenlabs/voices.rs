//! Voices (`GET /v2/voices`): the account's own and the default ones.

use super::{API, error};
use crate::http;
use koo_core::cloud::{CloudResult, Voice};
use serde_json::Value;

/// Of the voice's labels, the ones that say what it is like, in this order.
const TRAITS: [&str; 5] = ["gender", "age", "accent", "descriptive", "use_case"];
/// Pages of 100 read at most, for an account with a very long list.
const PAGES: usize = 20;

pub fn list(key: &str) -> CloudResult<Vec<Voice>> {
    let mut voices = Vec::new();
    let mut token: Option<String> = None;
    for _ in 0..PAGES {
        let mut url = format!("{API}/v2/voices?page_size=100&include_total_count=false");
        if let Some(token) = &token {
            url.push_str("&next_page_token=");
            url.push_str(&query_value(token));
        }
        let reply = http::get(&url, &[("xi-api-key", key)])?;
        if !reply.ok() {
            return Err(error(&reply));
        }
        let json = reply.json()?;
        let page = json["voices"].as_array().into_iter().flatten();
        voices.extend(page.filter_map(voice));
        token = json["next_page_token"].as_str().map(str::to_owned);
        if !json["has_more"].as_bool().unwrap_or(false) || token.is_none() {
            break;
        }
    }
    Ok(voices)
}

fn voice(json: &Value) -> Option<Voice> {
    let text = |key: &str| json[key].as_str().map(str::to_owned);
    let labels = &json["labels"];
    Some(Voice {
        id: text("voice_id")?,
        name: text("name")?,
        category: text("category").unwrap_or_default(),
        traits: TRAITS
            .iter()
            .filter_map(|t| labels[*t].as_str())
            .filter(|t| !t.is_empty())
            .map(|t| t.replace('_', " "))
            .collect(),
        sample: text("preview_url"),
    })
}

/// `text` as a value in a query string.
fn query_value(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            b => format!("%{b:02X}"),
        })
        .collect()
}
