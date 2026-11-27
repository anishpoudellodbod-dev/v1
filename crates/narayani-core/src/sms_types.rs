use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashSet;

#[allow(non_camel_case_types)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SmsType {
    PAID,
    PAYMENT_REMAINDER,
    URGENT,
    CUSTOM(String),
}

impl Default for SmsType {
    fn default() -> Self {
        Self::CUSTOM(String::new())
    }
}

impl SmsType {
    pub fn from_id(raw: &str) -> Self {
        let trimmed = raw.trim();
        let norm: String = trimmed
            .chars()
            .map(|c| {
                if c == ' ' || c == '-' {
                    '_'
                } else {
                    c.to_ascii_uppercase()
                }
            })
            .collect();
        match norm.as_str() {
            "PAID" | "PAYMENT_RECEIVED" => Self::PAID,
            "PAYMENT_REMAINDER" | "PAYMENT_REMINDER" => Self::PAYMENT_REMAINDER,
            "URGENT" | "URGENT_REMINDER" => Self::URGENT,
            _ => Self::CUSTOM(trimmed.to_string()),
        }
    }

    pub fn setting_template_ids(&self) -> Vec<&str> {
        match self {
            Self::PAID => vec!["PAID", "payment-received", "paid", "payment_received"],
            Self::PAYMENT_REMAINDER => vec![
                "PAYMENT REMAINDER",
                "PAYMENT REMINDER",
                "PAYMENT_REMAINDER",
                "PAYMENT_REMINDER",
                "payment-reminder",
                "payment-remainder",
                "payment_reminder",
                "payment_remainder",
            ],
            Self::URGENT => vec!["URGENT", "urgent-reminder", "urgent", "urgent_reminder"],
            Self::CUSTOM(id) => {
                let t = id.trim();
                if t.is_empty() {
                    Vec::new()
                } else {
                    vec![t]
                }
            }
        }
    }

    pub fn matches_template_id(&self, tpl_id: &str) -> bool {
        let target = tpl_id.trim();
        self.setting_template_ids()
            .into_iter()
            .any(|candidate| target.eq_ignore_ascii_case(candidate))
    }
}

impl Serialize for SmsType {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::PAID => serializer.serialize_str("PAID"),
            Self::PAYMENT_REMAINDER => serializer.serialize_str("PAYMENT_REMAINDER"),
            Self::URGENT => serializer.serialize_str("URGENT"),
            Self::CUSTOM(id) => serializer.serialize_str(id),
        }
    }
}

impl<'de> Deserialize<'de> for SmsType {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Ok(Self::from_id(&raw))
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SettingSmsTemplate {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default = "default_true")]
    pub active: bool,
    #[serde(default)]
    pub channel: Option<String>,
    pub content: String,
    #[serde(default)]
    pub category: Option<String>,
}

/// Extracts unique `{placeholder}` variable names in appearance order from an SMS template body.
pub fn extract_template_placeholders(content: &str) -> Vec<String> {
    let mut placeholders = Vec::new();
    let mut seen = HashSet::new();
    let mut rest = content;

    while let Some(open_idx) = rest.find('{') {
        let after_open = &rest[open_idx + 1..];
        let Some(close_idx) = after_open.find('}') else {
            break;
        };
        let candidate = &after_open[..close_idx];
        if let Some(nested_open) = candidate.rfind('{') {
            rest = &after_open[nested_open..];
            continue;
        }
        if !candidate.is_empty() && !candidate.chars().any(char::is_whitespace) {
            if seen.insert(candidate.to_string()) {
                placeholders.push(candidate.to_string());
            }
        }
        rest = &after_open[close_idx + 1..];
    }

    placeholders
}
