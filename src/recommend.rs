//! Scoring and itinerary building.
//!
//! Each interest phrase is scored against every session with a field-weighted
//! BM25 (title > stream > abstract), plus a bonus for an exact phrase hit.
//! Scores are normalised per phrase so every interest carries similar weight,
//! then combined with group weights (key topics > new > existing interests).
//! Presenter names are matched separately and mark a session as "must see".
//!
//! The itinerary is built greedily without clashes: pinned → must-see
//! presenters → keynotes → best matches → a few "something different" picks
//! from streams the person did not mention.

use crate::model::{Catalog, Kind, Session};
use crate::text::{fold, normalise_phrase, tokens};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Profile {
    #[serde(default)]
    pub existing: Vec<String>,
    #[serde(default)]
    pub new_interests: Vec<String>,
    #[serde(default)]
    pub topics: Vec<String>,
    #[serde(default)]
    pub presenters: Vec<String>,
    /// Days to plan ("YYYY-MM-DD"). Empty means all days.
    #[serde(default)]
    pub days: Vec<String>,
    /// 0 = none, 1 = a few, 2 = plenty of "something different" picks.
    #[serde(default = "default_discovery")]
    pub discovery: u8,
    /// Changes which discovery sessions are picked when re-shuffled.
    #[serde(default)]
    pub seed: u32,
}

fn default_discovery() -> u8 {
    1
}

impl Profile {
    pub fn is_empty(&self) -> bool {
        self.existing.is_empty() && self.new_interests.is_empty() && self.topics.is_empty() && self.presenters.is_empty()
    }
    fn clean(list: &[String]) -> Vec<String> {
        let mut seen = HashSet::new();
        list.iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty() && seen.insert(s.to_lowercase())).collect()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Reason {
    /// "topic" | "existing" | "new" | "presenter" | "company"
    pub kind: &'static str,
    pub label: String,
    #[serde(skip)]
    pub strength: f32,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Scored {
    pub score: f32,
    /// Score from interests and topics only (no presenter/company boost).
    #[serde(skip)]
    pub topical: f32,
    pub must_see: bool,
    pub reasons: Vec<Reason>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    /// "pinned" | "presenter" | "keynote" | "match" | "discovery"
    pub why: String,
    #[serde(default)]
    pub reasons: Vec<String>,
}

struct Doc {
    tf: HashMap<String, f32>,
    len: f32,
    title: String,
    body: String,
    /// Folded name tokens per speaker.
    speaker_tokens: Vec<Vec<String>>,
    companies: Vec<String>,
}

pub struct Index {
    docs: Vec<Doc>,
    idf: HashMap<String, f32>,
    avg_len: f32,
}

const K1: f32 = 1.2;
const B: f32 = 0.75;
/// Minimum denominator when normalising a phrase's scores, so that a phrase
/// with only weak matches does not get inflated to look like a strong one.
const NORM_FLOOR: f32 = 6.0;
const COUNT_AT: f32 = 0.15;
const REASON_AT: f32 = 0.3;

fn name_tokens(s: &str) -> Vec<String> {
    fold(s).split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()).map(String::from).collect()
}

impl Index {
    pub fn new(cat: &Catalog) -> Self {
        let mut docs = Vec::with_capacity(cat.sessions.len());
        let mut df: HashMap<String, f32> = HashMap::new();
        for s in &cat.sessions {
            let mut tf: HashMap<String, f32> = HashMap::new();
            let mut len = 0.0;
            let companies: Vec<String> = s.speakers.iter().map(|p| p.company.clone()).filter(|c| !c.is_empty()).collect();
            for (text, w) in [
                (s.title.as_str(), 3.0),
                (s.stream.as_str(), 2.0),
                (s.abstract_text.as_str(), 1.0),
                (&s.tags.join(" "), 1.0),
                (&companies.join(" "), 1.0),
            ] {
                for t in tokens(text) {
                    *tf.entry(t).or_default() += w;
                    len += w;
                }
            }
            for t in tf.keys() {
                *df.entry(t.clone()).or_default() += 1.0;
            }
            docs.push(Doc {
                tf,
                len,
                title: normalise_phrase(&s.title),
                body: normalise_phrase(&format!("{} {} {}", s.stream, s.abstract_text, s.tags.join(" "))),
                speaker_tokens: s.speakers.iter().map(|p| name_tokens(&p.name)).collect(),
                companies: companies.iter().map(|c| normalise_phrase(c)).collect(),
            });
        }
        let n = docs.len().max(1) as f32;
        let avg_len = docs.iter().map(|d| d.len).sum::<f32>() / n;
        let idf = df.into_iter().map(|(t, d)| (t, (1.0 + (n - d + 0.5) / (d + 0.5)).ln())).collect();
        Index { docs, idf, avg_len: avg_len.max(1.0) }
    }

    fn bm25(&self, i: usize, tok: &str) -> f32 {
        let d = &self.docs[i];
        let Some(&tf) = d.tf.get(tok) else { return 0.0 };
        let idf = self.idf.get(tok).copied().unwrap_or(0.0);
        idf * tf * (K1 + 1.0) / (tf + K1 * (1.0 - B + B * d.len / self.avg_len))
    }

    fn phrase_raw(&self, i: usize, phrase: &str) -> f32 {
        let mut toks = tokens(phrase);
        toks.sort();
        toks.dedup();
        if toks.is_empty() {
            return 0.0;
        }
        let mut sum = 0.0;
        let mut matched = 0;
        for t in &toks {
            let s = self.bm25(i, t);
            if s > 0.0 {
                matched += 1;
                sum += s;
            }
        }
        if matched == 0 || (toks.len() >= 2 && matched * 2 < toks.len()) {
            return 0.0;
        }
        let mut score = sum / (toks.len() as f32).sqrt();
        let np = normalise_phrase(phrase);
        if np.trim().len() >= 1 {
            let d = &self.docs[i];
            if d.title.contains(&np) {
                score += 4.0;
            } else if d.body.contains(&np) {
                score += 2.0;
            }
        }
        score
    }

    /// Raw relevance of a free-text interest, trying a few domain synonyms.
    fn interest_raw(&self, i: usize, phrase: &str) -> f32 {
        let mut best = self.phrase_raw(i, phrase);
        for syn in synonyms(phrase) {
            best = best.max(0.85 * self.phrase_raw(i, syn));
        }
        best
    }

    /// Does any speaker's name contain every token of `query` (prefix match)?
    fn speaker_match(&self, i: usize, query: &str, sessions: &[Session]) -> Option<String> {
        let q = name_tokens(query);
        if q.is_empty() {
            return None;
        }
        self.docs[i].speaker_tokens.iter().enumerate().find_map(|(k, name)| {
            let all = q.iter().all(|qt| name.iter().any(|nt| if qt.len() >= 3 { nt.starts_with(qt.as_str()) } else { nt == qt }));
            all.then(|| sessions[i].speakers[k].name.clone())
        })
    }

    fn company_match(&self, i: usize, query: &str, sessions: &[Session]) -> Option<String> {
        let q = normalise_phrase(query);
        if q.trim().len() < 3 {
            return None;
        }
        self.docs[i].companies.iter().position(|c| c.contains(&q)).map(|k| {
            sessions[i].speakers.iter().map(|s| s.company.clone()).filter(|c| !c.is_empty()).nth(k).unwrap_or_default()
        })
    }

    /// Score every session for a profile. Result is indexed like `cat.sessions`.
    pub fn score(&self, cat: &Catalog, profile: &Profile) -> Vec<Scored> {
        let n = cat.sessions.len();
        let mut out = vec![Scored::default(); n];
        let groups: [(&[String], &'static str, f32); 3] = [
            (&profile.topics, "topic", 1.5),
            (&profile.new_interests, "new", 1.2),
            (&profile.existing, "existing", 1.0),
        ];
        for (list, kind, weight) in groups {
            for phrase in Profile::clean(list) {
                let raw: Vec<f32> = (0..n).map(|i| if cat.sessions[i].kind == Kind::Logistics { 0.0 } else { self.interest_raw(i, &phrase) }).collect();
                let denom = raw.iter().cloned().fold(0.0, f32::max).max(NORM_FLOOR);
                for i in 0..n {
                    let norm = raw[i] / denom;
                    if norm >= COUNT_AT {
                        out[i].score += weight * norm;
                        out[i].topical += weight * norm;
                        if norm >= REASON_AT {
                            out[i].reasons.push(Reason { kind, label: phrase.clone(), strength: weight * norm });
                        }
                    }
                }
            }
        }
        for p in Profile::clean(&profile.presenters) {
            let person_hits: Vec<Option<String>> = (0..n).map(|i| self.speaker_match(i, &p, &cat.sessions)).collect();
            let any_person = person_hits.iter().any(Option::is_some);
            for i in 0..n {
                if let Some(name) = &person_hits[i] {
                    out[i].score += 10.0;
                    out[i].must_see = true;
                    out[i].reasons.push(Reason { kind: "presenter", label: name.clone(), strength: 100.0 });
                } else if !any_person {
                    if let Some(company) = self.company_match(i, &p, &cat.sessions) {
                        out[i].score += 2.0;
                        out[i].reasons.push(Reason { kind: "company", label: company, strength: 50.0 });
                    }
                }
            }
        }
        // Strongest reasons first; keep the list short enough to scan.
        for sc in out.iter_mut() {
            sc.reasons.sort_by(|a, b| b.strength.total_cmp(&a.strength));
            sc.reasons.truncate(3);
        }
        out
    }

    /// Free-text search used by the chat tools: topical relevance plus
    /// presenter/company name hits. Returns (index, score) best first.
    pub fn search(&self, cat: &Catalog, query: &str, day: Option<&str>, limit: usize) -> Vec<(usize, f32)> {
        let mut hits: Vec<(usize, f32)> = (0..cat.sessions.len())
            .filter(|&i| cat.sessions[i].kind != Kind::Logistics)
            .filter(|&i| day.map_or(true, |d| cat.sessions[i].day == d))
            .map(|i| {
                let mut s = self.interest_raw(i, query);
                if self.speaker_match(i, query, &cat.sessions).is_some() {
                    s += 20.0;
                }
                if self.company_match(i, query, &cat.sessions).is_some() {
                    s += 5.0;
                }
                if cat.sessions[i].key.eq_ignore_ascii_case(query.trim()) {
                    s += 50.0;
                }
                (i, s)
            })
            .filter(|&(_, s)| s > 0.0)
            .collect();
        hits.sort_by(|a, b| b.1.total_cmp(&a.1));
        hits.truncate(limit);
        hits
    }
}

/// A few conference-domain synonyms and expansions.
fn synonyms(phrase: &str) -> &'static [&'static str] {
    let p = fold(phrase);
    match p.trim() {
        "ai" | "a.i." => &["artificial intelligence", "machine learning", "large language model", "llm", "generative ai", "agentic"],
        "artificial intelligence" => &["ai", "machine learning"],
        "llm" | "llms" | "large language models" | "large language model" => &["large language model", "llm", "generative ai", "genai", "gpt"],
        "genai" | "gen ai" | "generative ai" => &["generative ai", "llm", "large language model", "genai"],
        "ml" | "machine learning" => &["machine learning", "ai", "predictive model"],
        "agents" | "agentic" | "agentic ai" | "ai agents" => &["agentic", "agent", "agents"],
        "r" => &["r package", "shiny", "tidyverse", "pharmaverse", "admiral", "rstudio", "posit"],
        "python" => &["pandas", "jupyter", "pyspark"],
        "open source" | "open-source" => &["open source", "r", "python", "pharmaverse", "github"],
        "cdisc" => &["sdtm", "adam", "define xml", "cdisc standards"],
        "sdtm" => &["cdisc", "tabulation"],
        "adam" => &["cdisc", "analysis dataset"],
        "rwe" | "rwd" | "real world evidence" | "real world data" | "real-world evidence" | "real-world data" => {
            &["real world evidence", "real world data", "observational", "registry"]
        }
        "dataviz" | "data viz" | "visualisation" | "visualization" | "data visualisation" | "data visualization" => {
            &["visualisation", "graphics", "plots", "dashboard", "figures"]
        }
        "automation" => &["automate", "automated", "pipeline", "workflow"],
        "leadership" => &["leader", "leaders", "management", "people leadership"],
        "regulatory" | "submissions" | "regulatory submissions" => &["submission", "fda", "ema", "regulatory", "agency"],
        "usdm" | "digital protocol" => &["usdm", "digital protocol", "schedule of activities"],
        "tfl" | "tfls" | "tlf" | "tlfs" => &["tfl", "tlf", "tables listings figures", "tables"],
        "statistics" | "stats" => &["statistical", "statistician", "estimand", "bayesian"],
        "cloud" => &["aws", "azure", "cloud"],
        "career" | "careers" | "professional development" => &["career", "skills", "mentoring", "professional development"],
        "gxp" | "validation" => &["validation", "validated", "gxp", "qualification"],
        _ => &[],
    }
}

fn hash32(seed: u32, s: &str) -> f32 {
    let mut h: u32 = 0x811c_9dc5 ^ seed.wrapping_mul(0x9e37_79b9);
    for b in s.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    (h % 10_000) as f32 / 10_000.0
}

pub struct Built {
    pub items: Vec<Item>,
    pub scores: Vec<Scored>,
}

/// Build an itinerary for `profile`, keeping `pinned` session ids.
pub fn build(cat: &Catalog, idx: &Index, profile: &Profile, pinned: &[String]) -> Built {
    let scores = idx.score(cat, profile);
    let ss = &cat.sessions;
    let day_ok = |s: &Session| profile.days.is_empty() || profile.days.iter().any(|d| d == &s.day);
    let eligible = |s: &Session| !matches!(s.kind, Kind::Logistics | Kind::Social) && s.start < s.end && day_ok(s);

    let mut chosen: Vec<usize> = Vec::new();
    let mut items: Vec<Item> = Vec::new();
    let try_add = |i: usize, why: &str, reasons: Vec<String>, chosen: &mut Vec<usize>, items: &mut Vec<Item>| -> bool {
        if chosen.contains(&i) || chosen.iter().any(|&j| ss[j].clashes(&ss[i])) {
            return false;
        }
        chosen.push(i);
        items.push(Item { id: ss[i].id.clone(), why: why.into(), reasons });
        true
    };
    let reason_strings = |i: usize| -> Vec<String> {
        scores[i]
            .reasons
            .iter()
            .map(|r| match r.kind {
                "topic" => format!("Key topic: {}", r.label),
                "new" => format!("New interest: {}", r.label),
                "existing" => format!("Interest: {}", r.label),
                "presenter" => format!("Presenter: {}", r.label),
                "company" => format!("Company: {}", r.label),
                _ => r.label.clone(),
            })
            .collect()
    };

    // 1. Sessions the person has already chosen.
    for id in pinned {
        if let Some(i) = ss.iter().position(|s| &s.id == id) {
            let mut r = reason_strings(i);
            r.insert(0, "Chosen by you".into());
            try_add(i, "pinned", r, &mut chosen, &mut items);
        }
    }

    // 2. Must-see presenters, best scoring first.
    let mut must: Vec<usize> = (0..ss.len()).filter(|&i| scores[i].must_see && eligible(&ss[i])).collect();
    must.sort_by(|&a, &b| scores[b].score.total_cmp(&scores[a].score));
    for i in must {
        try_add(i, "presenter", reason_strings(i), &mut chosen, &mut items);
    }

    // 3. Plenary keynotes.
    for i in 0..ss.len() {
        if ss[i].kind == Kind::Keynote && eligible(&ss[i]) {
            let mut r = vec!["Plenary keynote".to_string()];
            r.extend(reason_strings(i));
            try_add(i, "keynote", r, &mut chosen, &mut items);
        }
    }

    // 4. Best matches, in two passes: strong matches now, weaker ones after
    //    the "something different" picks so those still find room.
    let top = (0..ss.len()).filter(|&i| eligible(&ss[i])).map(|i| scores[i].topical).fold(0.0, f32::max);
    let threshold = (0.2 * top).max(0.3);
    let strong = (0.45 * top).max(threshold);
    let mut posters = 0usize;
    let add_matches = |min: f32, chosen: &mut Vec<usize>, items: &mut Vec<Item>, posters: &mut usize| {
        let mut cands: Vec<usize> = (0..ss.len()).filter(|&i| eligible(&ss[i]) && scores[i].score >= min).collect();
        cands.sort_by(|&a, &b| scores[b].score.total_cmp(&scores[a].score));
        for i in cands {
            let mut r = reason_strings(i);
            if r.is_empty() {
                r.push("Related to your interests".into());
            }
            if ss[i].kind == Kind::Poster {
                if *posters < 6 && try_add(i, "match", r, chosen, items) {
                    *posters += 1;
                }
            } else {
                try_add(i, "match", r, chosen, items);
            }
        }
    };
    add_matches(strong, &mut chosen, &mut items, &mut posters);

    // 5. Something different: streams the person showed no interest in.
    let interest_streams: HashSet<&str> = (0..ss.len())
        .filter(|&i| scores[i].score >= strong)
        .map(|i| ss[i].stream_code.as_str())
        .collect();
    let days: Vec<String> = {
        let mut d: Vec<String> = ss.iter().filter(|s| eligible(s) && matches!(s.kind, Kind::Talk)).map(|s| s.day.clone()).collect();
        d.sort();
        d.dedup();
        d
    };
    let per_day = match profile.discovery {
        0 => 0,
        1 => 1,
        _ => 3,
    };
    let mut used_streams: HashSet<String> = HashSet::new();
    for round in 0..per_day {
        for day in &days {
            let mut best: Option<(usize, f32)> = None;
            for i in 0..ss.len() {
                let s = &ss[i];
                if &s.day != day || !matches!(s.kind, Kind::Talk | Kind::Workshop) || !eligible(s) || chosen.contains(&i) {
                    continue;
                }
                if scores[i].score >= threshold || s.stream_code.is_empty() || used_streams.contains(&s.stream_code) {
                    continue;
                }
                if chosen.iter().any(|&j| ss[j].clashes(s)) {
                    continue;
                }
                let mut d = 1.0 + hash32(profile.seed.wrapping_add(round as u32), &s.id);
                if !interest_streams.contains(s.stream_code.as_str()) {
                    d += 1.0;
                }
                if s.level == "Beginner" {
                    d += 0.4;
                }
                if s.kind == Kind::Workshop {
                    d += 0.2;
                }
                if best.map_or(true, |(_, b)| d > b) {
                    best = Some((i, d));
                }
            }
            if let Some((i, _)) = best {
                let s = &ss[i];
                let mut r = vec![format!("Something different: {}", s.stream)];
                if s.level == "Beginner" {
                    r.push("Beginner-friendly".into());
                }
                used_streams.insert(s.stream_code.clone());
                try_add(i, "discovery", r, &mut chosen, &mut items);
            }
        }
    }

    // 6. Fill remaining time with weaker matches.
    add_matches(threshold, &mut chosen, &mut items, &mut posters);

    items.sort_by(|a, b| {
        let (sa, sb) = (cat.sessions.iter().find(|s| s.id == a.id), cat.sessions.iter().find(|s| s.id == b.id));
        match (sa, sb) {
            (Some(x), Some(y)) => x.start.cmp(&y.start).then(x.title.cmp(&y.title)),
            _ => std::cmp::Ordering::Equal,
        }
    });
    Built { items, scores }
}
