//! Normalised agenda data model, independent of Cvent's raw shape.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Speaker {
    pub name: String,
    pub company: String,
    #[serde(default)]
    pub job_title: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Talk,
    Workshop,
    Poster,
    Keynote,
    Social,
    Logistics,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    /// Short human/LLM-friendly reference: the session code (e.g. "ET11")
    /// when it is unique, otherwise the first 8 characters of the id.
    pub key: String,
    pub code: String,
    pub title: String,
    /// Stream name without the code suffix, e.g. "Machine Learning, AI & Large Language Models".
    pub stream: String,
    /// Stream code, e.g. "ML". Empty for logistics/social items.
    pub stream_code: String,
    pub kind: Kind,
    /// "YYYY-MM-DD"
    pub day: String,
    /// "YYYY-MM-DDTHH:MM" in event local time (Glasgow, GMT in November).
    pub start: String,
    pub end: String,
    pub room: String,
    #[serde(rename = "abstract")]
    pub abstract_text: String,
    pub speakers: Vec<Speaker>,
    /// Audience level: "Beginner", "Intermediate", "Advanced" or empty.
    pub level: String,
    pub tags: Vec<String>,
}

impl Session {
    pub fn start_time(&self) -> &str {
        self.start.get(11..16).unwrap_or("")
    }
    pub fn end_time(&self) -> &str {
        self.end.get(11..16).unwrap_or("")
    }

    /// Whether attending both sessions is impossible. Posters are all shown
    /// together in one poster session, so posters never clash with each other.
    pub fn clashes(&self, other: &Session) -> bool {
        if self.id == other.id {
            return false;
        }
        if self.kind == Kind::Poster && other.kind == Kind::Poster {
            return false;
        }
        if self.start >= self.end || other.start >= other.end {
            return false;
        }
        self.start < other.end && other.start < self.end
    }

    pub fn speakers_line(&self) -> String {
        self.speakers
            .iter()
            .map(|s| if s.company.is_empty() { s.name.clone() } else { format!("{} ({})", s.name, s.company) })
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// One-line summary used in LLM tool results and the system catalogue.
    pub fn summary_line(&self) -> String {
        let mut s = format!(
            "[{}] {} {}–{} · {} · {} · {}",
            self.key,
            day_label(&self.day),
            self.start_time(),
            self.end_time(),
            if self.room.is_empty() { "TBC" } else { &self.room },
            if self.stream_code.is_empty() { kind_label(self.kind).to_string() } else { format!("{} ({})", self.stream, self.stream_code) },
            self.title
        );
        let sp = self.speakers_line();
        if !sp.is_empty() {
            s.push_str(" — ");
            s.push_str(&sp);
        }
        if !self.level.is_empty() {
            s.push_str(&format!(" · {}", self.level));
        }
        s
    }
}

pub fn kind_label(k: Kind) -> &'static str {
    match k {
        Kind::Talk => "Talk",
        Kind::Workshop => "Hands-on workshop",
        Kind::Poster => "Poster",
        Kind::Keynote => "Keynote",
        Kind::Social => "Social",
        Kind::Logistics => "Logistics",
    }
}

pub fn day_label(day: &str) -> String {
    match day {
        "2026-11-15" => "Sun 15 Nov".into(),
        "2026-11-16" => "Mon 16 Nov".into(),
        "2026-11-17" => "Tue 17 Nov".into(),
        "2026-11-18" => "Wed 18 Nov".into(),
        other => other.to_string(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Catalog {
    pub event_name: String,
    pub source_url: String,
    pub fetched_at: String,
    pub sessions: Vec<Session>,
}

impl Catalog {
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&text)?)
    }

    pub fn save(&self, path: &str) -> anyhow::Result<()> {
        if let Some(dir) = std::path::Path::new(path).parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    /// Resolve a reference typed by a person or the model: a session key,
    /// code, full id or id prefix (case-insensitive).
    pub fn find(&self, reference: &str) -> Option<&Session> {
        let r = reference.trim().trim_matches(|c| c == '[' || c == ']').to_ascii_lowercase();
        if r.is_empty() {
            return None;
        }
        self.sessions
            .iter()
            .find(|s| s.key.to_ascii_lowercase() == r || s.id == r)
            .or_else(|| self.sessions.iter().find(|s| !s.code.is_empty() && s.code.to_ascii_lowercase() == r))
            .or_else(|| if r.len() >= 6 { self.sessions.iter().find(|s| s.id.starts_with(&r)) } else { None })
    }

    pub fn by_id(&self) -> HashMap<&str, &Session> {
        self.sessions.iter().map(|s| (s.id.as_str(), s)).collect()
    }

    pub fn days(&self) -> Vec<String> {
        let mut d: Vec<String> = self.sessions.iter().map(|s| s.day.clone()).collect();
        d.sort();
        d.dedup();
        d
    }
}
