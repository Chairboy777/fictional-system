//! Fetches the live agenda from the Cvent event site that hosts the PHUSE
//! EU Connect agenda, and normalises it into our own `Catalog`.
//!
//! The agenda page is a client-rendered app. It embeds the event id and the
//! current "snapshot versions" in its HTML, then pages through a public
//! GraphQL `Sessions` query. We do the same.

use crate::model::{Catalog, Kind, Session, Speaker};
use crate::text::html_to_text;
use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use std::collections::HashMap;

pub const AGENDA_URL: &str = "https://events.phuse.global/event/EUConnect26/agenda";
const GRAPHQL_URL: &str = "https://web-eur.cvent.com/event/graphql";
const SESSIONS_QUERY: &str = include_str!("sessions.graphql");
const USER_AGENT: &str = "Mozilla/5.0 (compatible; conference-itinerary-planner/0.1)";

/// Pull `"key":"value"` out of the page. The values sit inside an escaped
/// JSON string in the Next.js payload, so try both escaped and plain forms.
fn extract(html: &str, key: &str) -> Option<String> {
    for (open, close) in [(format!("\\\"{key}\\\":\\\""), "\\\""), (format!("\"{key}\":\""), "\"")] {
        if let Some(pos) = html.find(&open) {
            let rest = &html[pos + open.len()..];
            if let Some(end) = rest.find(close) {
                let v = &rest[..end];
                if !v.is_empty() && v.len() < 100 {
                    return Some(v.to_string());
                }
            }
        }
    }
    None
}

pub async fn fetch_catalog(http: &reqwest::Client) -> Result<Catalog> {
    let html = http
        .get(AGENDA_URL)
        .header("User-Agent", USER_AGENT)
        .send()
        .await
        .context("requesting agenda page")?
        .error_for_status()?
        .text()
        .await?;

    let event_id = extract(&html, "eventId").ok_or_else(|| anyhow!("eventId not found in agenda page"))?;
    let environment = extract(&html, "environment").unwrap_or_else(|| "production-eu".into());
    let event_snap = extract(&html, "eventSnapshotVersion").ok_or_else(|| anyhow!("eventSnapshotVersion not found"))?;
    let account_snap = extract(&html, "accountSnapshotVersion");

    let mut raw: Vec<Value> = Vec::new();
    let mut token = String::new();
    for _page in 0..50 {
        let body = json!([{
            "operationName": "Sessions",
            "query": SESSIONS_QUERY,
            "variables": {
                "regTypeId": null,
                "token": token,
                "limit": 50,
                "sort": "startDateOnly:asc,startTimeOnly:asc,featuredSession:desc,displayPriority:desc,name:asc",
                "filter": "showOnAgenda eq 1 and status ne 7",
                "timezone": "85",
                "sessionCustomFieldsFilter": [],
                "limitByEmptyRegistration": true,
                "locale": "en-US",
                "resolveAccountTranslations": true,
                "resolveTagTranslations": true,
                "eventId": event_id,
                "environment": environment,
                "eventSnapshotVersion": event_snap,
                "accountSnapshotVersion": account_snap,
            }
        }]);
        let resp: Value = http
            .post(GRAPHQL_URL)
            .header("User-Agent", USER_AGENT)
            .header("Origin", "https://events.phuse.global")
            .json(&body)
            .send()
            .await
            .context("requesting sessions page")?
            .error_for_status()?
            .json()
            .await?;
        let sessions = &resp[0]["data"]["event"]["sessions"];
        let data = sessions["data"].as_array().ok_or_else(|| anyhow!("unexpected GraphQL response: {}", truncate(&resp.to_string(), 300)))?;
        raw.extend(data.iter().cloned());
        match sessions["paginationMetadata"]["nextToken"].as_str() {
            Some(t) if !t.is_empty() => token = t.to_string(),
            _ => break,
        }
    }

    let mut sessions: Vec<Session> = raw.iter().filter_map(normalise).collect();
    sessions.sort_by(|a, b| a.start.cmp(&b.start).then(a.room.cmp(&b.room)).then(a.title.cmp(&b.title)));
    assign_keys(&mut sessions);

    Ok(Catalog {
        event_name: "PHUSE EU Connect 2026".into(),
        source_url: AGENDA_URL.into(),
        fetched_at: now_utc(),
        sessions,
    })
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn str_of<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

/// Split "Open Source Technologies (OS)" into ("Open Source Technologies", "OS").
fn split_stream(name: &str) -> (String, String) {
    let n = name.trim();
    if n.ends_with(')') {
        if let Some(open) = n.rfind('(') {
            let code = &n[open + 1..n.len() - 1];
            if !code.is_empty() && code.len() <= 5 && code.chars().all(|c| c.is_ascii_alphabetic()) {
                return (n[..open].trim().to_string(), code.to_string());
            }
        }
    }
    (n.to_string(), String::new())
}

fn normalise(v: &Value) -> Option<Session> {
    let id = str_of(v, "id").to_string();
    let title = str_of(v, "name").trim().to_string();
    if id.is_empty() || title.is_empty() {
        return None;
    }
    let start = str_of(v, "startDateTime").get(..16).unwrap_or("").to_string();
    let end = str_of(v, "endDateTime").get(..16).unwrap_or("").to_string();
    let category = v["category"]["name"].as_str().unwrap_or("").trim();
    let (stream, stream_code) = split_stream(category);
    let ptype = str_of(v, "presentationType");

    let kind = match category {
        "" => Kind::Logistics,
        c if c.starts_with("Social") => Kind::Social,
        c if c.starts_with("Keynote") => Kind::Keynote,
        c if c.starts_with("Hands-on") => Kind::Workshop,
        c if c.starts_with("Poster") => Kind::Poster,
        _ if ptype == "Keynote" => Kind::Keynote,
        _ => Kind::Talk,
    };

    let mut level = String::new();
    let mut tags = Vec::new();
    for t in v["contentTags"].as_array().into_iter().flatten() {
        let text = str_of(t, "contentTagText").trim();
        if text.contains("Audience Level") {
            level = text.trim_start_matches('*').replace("Audience Level", "").trim().to_string();
        } else if !text.is_empty() {
            tags.push(text.to_string());
        }
    }

    // Speaker order comes from `speakerIds`; keep only speakers shown on the website.
    let order: HashMap<&str, i64> = v["speakerIds"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|s| (str_of(s, "id"), s["sessionSpeakerOrder"].as_i64().unwrap_or(99)))
        .collect();
    let mut sp: Vec<(i64, Speaker)> = v["speakers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|s| s["displayOnWebsite"].as_bool().unwrap_or(true))
        .map(|s| {
            let name = format!("{} {}", str_of(s, "firstName").trim(), str_of(s, "lastName").trim()).trim().to_string();
            (
                *order.get(str_of(s, "id")).unwrap_or(&99),
                Speaker { name, company: str_of(s, "company").trim().to_string(), job_title: str_of(s, "title").trim().to_string() },
            )
        })
        .collect();
    sp.sort_by_key(|(o, _)| *o);

    let room = v["sessionLocation"]["locationName"].as_str().unwrap_or("").trim().to_string();
    Some(Session {
        key: String::new(),
        code: str_of(v, "code").trim().to_string(),
        day: start.get(..10).unwrap_or("").to_string(),
        abstract_text: html_to_text(str_of(v, "description")),
        speakers: sp.into_iter().map(|(_, s)| s).collect(),
        id,
        title,
        stream,
        stream_code,
        kind,
        start,
        end,
        room,
        level,
        tags,
    })
}

/// Give every session a short, unique key. Official session codes (e.g.
/// "ET11") are used when unique; others get a kind prefix and a number in
/// chronological order: KN1 (keynote), HW1 (workshop), SOC1 (social),
/// INFO1 (breaks/logistics), S1 (anything else).
fn assign_keys(sessions: &mut [Session]) {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for s in sessions.iter() {
        if !s.code.is_empty() {
            *counts.entry(s.code.to_ascii_uppercase()).or_default() += 1;
        }
    }
    let mut used: std::collections::HashSet<String> = counts.iter().filter(|(_, n)| **n == 1).map(|(c, _)| c.clone()).collect();
    let mut next: HashMap<&'static str, usize> = HashMap::new();
    for s in sessions.iter_mut() {
        if !s.code.is_empty() && counts[&s.code.to_ascii_uppercase()] == 1 {
            s.key = s.code.clone();
            continue;
        }
        let prefix = match s.kind {
            Kind::Keynote => "KN",
            Kind::Workshop => "HW",
            Kind::Social => "SOC",
            Kind::Logistics => "INFO",
            _ => "S",
        };
        loop {
            let n = next.entry(prefix).or_insert(0);
            *n += 1;
            let key = format!("{prefix}{n}");
            if used.insert(key.to_ascii_uppercase()) {
                s.key = key;
                break;
            }
        }
    }
}

/// Current UTC time as "YYYY-MM-DD HH:MM UTC", without a date crate.
pub fn now_utc() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02} UTC", rem / 3600, (rem % 3600) / 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_escaped_values() {
        let html = r#"x \"eventSnapshotVersion\":\"abc123\",\"y\":1"#;
        assert_eq!(extract(html, "eventSnapshotVersion").as_deref(), Some("abc123"));
    }

    #[test]
    fn splits_stream_codes() {
        assert_eq!(split_stream("Open Source Technologies (OS)"), ("Open Source Technologies".into(), "OS".into()));
        assert_eq!(split_stream("Keynote Speakers"), ("Keynote Speakers".into(), String::new()));
    }
}
