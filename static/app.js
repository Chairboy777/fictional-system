// Connect Planner — client.
// State lives in the browser (localStorage); the Rust server is stateless.
"use strict";

const $ = (s, el = document) => el.querySelector(s);
const STORE_KEY = "connect-planner-v1";

const state = {
  meta: null,
  sessions: [],
  byId: new Map(),
  byKey: new Map(),
  profile: null,       // {existing, new_interests, topics, presenters, days, discovery, seed}
  items: [],           // [{id, why, reasons}]
  scores: {},          // id -> {score, must_see, reasons}
  transcript: [],      // raw Claude API messages, replayed each turn
  chatLog: [],         // [{role, text, actions, error}]
  day: null,
  browsing: false,
  lastField: "existing",
};

// ---------- persistence ----------
function save() {
  try {
    const { profile, items, scores, transcript, chatLog, day } = state;
    localStorage.setItem(STORE_KEY, JSON.stringify({ profile, items, scores, transcript, chatLog, day }));
  } catch { /* storage unavailable: app still works for this visit */ }
}
function load() {
  try {
    const raw = localStorage.getItem(STORE_KEY);
    if (!raw) return;
    const s = JSON.parse(raw);
    Object.assign(state, {
      profile: s.profile || null,
      items: s.items || [],
      scores: s.scores || {},
      transcript: s.transcript || [],
      chatLog: s.chatLog || [],
      day: s.day || null,
    });
  } catch { /* ignore */ }
}

// ---------- helpers ----------
const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));
const hm = (iso) => (iso || "").slice(11, 16);
const dayLabel = (d) => (state.meta?.days.find((x) => x.date === d)?.label) || d;
const clashes = (a, b) => {
  if (a.id === b.id) return false;
  if (a.kind === "poster" && b.kind === "poster") return false;
  if (!(a.start < a.end) || !(b.start < b.end)) return false;
  return a.start < b.end && b.start < a.end;
};
const inPlan = (id) => state.items.some((i) => i.id === id);
const scoreOf = (id) => state.scores[id]?.score || 0;
const hue = (code) => { let h = 0; for (const c of code || "x") h = (h * 31 + c.charCodeAt(0)) % 360; return h; };
const splitList = (v) => v.split(/[,;\n]+/).map((x) => x.trim()).filter(Boolean);

function toast(msg) {
  const t = $("#toast");
  t.textContent = msg;
  t.hidden = false;
  clearTimeout(toast._t);
  toast._t = setTimeout(() => (t.hidden = true), 2600);
}

async function api(path, body) {
  const res = await fetch(path, body === undefined ? {} : {
    method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body),
  });
  if (!res.ok) throw new Error(`${res.status} ${await res.text()}`);
  return res.json();
}

function sortItems() {
  state.items.sort((a, b) => {
    const sa = state.byId.get(a.id), sb = state.byId.get(b.id);
    return (sa?.start || "").localeCompare(sb?.start || "") || (sa?.title || "").localeCompare(sb?.title || "");
  });
}

/** Add a session the person picked; drop anything that clashes. */
function addSession(id, note = "Chosen by you") {
  const s = state.byId.get(id);
  if (!s) return;
  const removed = state.items.filter((i) => clashes(state.byId.get(i.id), s));
  state.items = state.items.filter((i) => !removed.includes(i));
  const existing = state.items.find((i) => i.id === id);
  if (existing) existing.why = "pinned";
  else state.items.push({ id, why: "pinned", reasons: [note, ...reasonLabels(id)] });
  sortItems();
  save();
  render();
  toast(removed.length ? `Added ${s.key}; removed ${removed.map((r) => state.byId.get(r.id).key).join(", ")}` : `Added ${s.key}`);
}
function removeSession(id) {
  state.items = state.items.filter((i) => i.id !== id);
  save();
  render();
}
function reasonLabels(id) {
  const names = { topic: "Key topic", new: "New interest", existing: "Interest", presenter: "Presenter", company: "Company" };
  return (state.scores[id]?.reasons || []).map((r) => `${names[r.kind] || r.kind}: ${r.label}`);
}

// ---------- profile form ----------
const FIELDS = { existing: "#f_existing", new_interests: "#f_new", topics: "#f_topics", presenters: "#f_presenters" };

function fillForm() {
  const p = state.profile || {};
  for (const [k, sel] of Object.entries(FIELDS)) $(sel).value = (p[k] || []).join(", ");
  const days = p.days?.length ? p.days : state.meta.days.map((d) => d.date);
  document.querySelectorAll("#dayChecks input").forEach((c) => (c.checked = days.includes(c.value)));
  const disc = String(p.discovery ?? 1);
  document.querySelectorAll('input[name="discovery"]').forEach((r) => (r.checked = r.value === disc));
  const pinned = state.items.filter((i) => i.why === "pinned").length;
  $("#keepNote").textContent = pinned ? `${pinned} session${pinned > 1 ? "s" : ""} you chose will be kept.` : "";
}

function readForm() {
  const p = { seed: state.profile?.seed || 0 };
  for (const [k, sel] of Object.entries(FIELDS)) p[k] = splitList($(sel).value);
  const all = state.meta.days.map((d) => d.date);
  const days = [...document.querySelectorAll("#dayChecks input:checked")].map((c) => c.value);
  p.days = days.length === all.length ? [] : days;
  p.discovery = Number(document.querySelector('input[name="discovery"]:checked')?.value ?? 1);
  return p;
}

async function buildItinerary(profile) {
  const pinned = state.items.filter((i) => i.why === "pinned").map((i) => i.id);
  const res = await api("/api/recommend", { profile, pinned });
  state.profile = profile;
  state.items = res.items;
  state.scores = res.scores;
  save();
}

async function refreshScores() {
  try {
    const res = await api("/api/recommend", { profile: state.profile, pinned: [] });
    state.scores = res.scores;
  } catch { /* keep old scores */ }
}

// ---------- rendering ----------
function showSetup(show) {
  $("#setup").hidden = !show;
  $("#workspace").hidden = show;
  $("#editProfileBtn").hidden = show || !state.profile;
  if (show) fillForm();
}

function render() {
  renderTabs();
  if (state.browsing) renderBrowse();
  renderDay();
}

function renderTabs() {
  const tabs = $("#dayTabs");
  const days = state.meta.days.filter((d) => !state.profile?.days?.length || state.profile.days.includes(d.date));
  if (!days.some((d) => d.date === state.day)) {
    state.day = days.find((d) => state.items.some((i) => state.byId.get(i.id)?.day === d.date))?.date || days[0]?.date;
  }
  tabs.innerHTML = days.map((d) => {
    const n = state.items.filter((i) => state.byId.get(i.id)?.day === d.date).length;
    return `<button class="tab" role="tab" aria-selected="${d.date === state.day}" data-day="${d.date}">${esc(d.label)}<span class="count">${n}</span></button>`;
  }).join("");
}

function sessionMeta(s) {
  const bits = [];
  if (s.stream_code) bits.push(`<span class="badge" style="--h:${hue(s.stream_code)}" title="${esc(s.stream)}">${esc(s.stream_code)} · ${esc(s.stream)}</span>`);
  else bits.push(`<span class="badge" style="--h:150">${esc(kindLabel(s.kind))}</span>`);
  if (s.room) bits.push(`<span>${esc(s.room)}</span>`);
  bits.push(`<span>${esc(s.key)}</span>`);
  if (s.level) bits.push(`<span>${esc(s.level)}</span>`);
  return bits.join("");
}
const kindLabel = (k) => ({ talk: "Talk", workshop: "Hands-on workshop", poster: "Poster", keynote: "Keynote", social: "Social", logistics: "Logistics" }[k] || k);

function speakersHtml(s) {
  if (!s.speakers.length) return "";
  return `<div class="speakers">${s.speakers.map((p) => `${esc(p.name)}${p.company ? ` <span class="co">(${esc(p.company)})</span>` : ""}`).join(", ")}</div>`;
}
function flagsHtml(s) {
  return s.tags.filter((t) => /pre-registration|limited capacity|panel|interactive/i.test(t)).map((t) => `<span class="flag">${esc(t)}</span>`).join("");
}

function cardHtml(item) {
  const s = state.byId.get(item.id);
  if (!s) return "";
  const alts = state.sessions
    .filter((o) => o.kind !== "logistics" && o.kind !== "social" && clashes(o, s) && !inPlan(o.id))
    .sort((a, b) => scoreOf(b.id) - scoreOf(a.id) || a.start.localeCompare(b.start));
  return `
  <article class="card why-${esc(item.why)}" data-id="${s.id}">
    <div class="card-top">${sessionMeta(s)}</div>
    <h3><button data-open="${s.id}">${esc(s.title)}</button></h3>
    ${speakersHtml(s)}
    <div class="reasons">${(item.reasons || []).map((r) => `<span class="reason">${esc(r)}</span>`).join("")}${flagsHtml(s)}</div>
    ${s.abstract ? `<details class="abstract"><summary>Abstract</summary><p class="abstract-text">${esc(s.abstract)}</p></details>` : ""}
    <div class="card-actions small">
      ${alts.length ? `<button data-alts="${s.id}">Alternatives (${alts.length})</button>` : ""}
      <button class="danger" data-remove="${s.id}">Remove</button>
    </div>
    <div class="alts" id="alts-${s.id}" hidden>
      ${alts.slice(0, 12).map((o) => altHtml(o, "Swap")).join("")}
    </div>
  </article>`;
}

function altHtml(o, verb = "Add") {
  return `<div class="alt">
    <div><div class="t"><button class="link" data-open="${o.id}">${esc(o.title)}</button></div>
    <div class="m">${hm(o.start)}–${hm(o.end)} · ${esc(o.stream_code || kindLabel(o.kind))} · ${esc(o.room)}${o.speakers.length ? " · " + esc(o.speakers.map((p) => p.name + (p.company ? ` (${p.company})` : "")).join(", ")) : ""}${scoreOf(o.id) > 0.3 ? " · <strong>relevant</strong>" : ""}</div></div>
    ${inPlan(o.id) ? `<span class="small muted">In plan</span>` : `<button class="small" data-add="${o.id}">${verb}</button>`}
  </div>`;
}

/** Build the day timeline: chosen sessions, breaks, and open slots. */
function dayRows(day) {
  const rows = [];
  const chosen = state.items.map((i) => ({ item: i, s: state.byId.get(i.id) })).filter((x) => x.s && x.s.day === day);
  for (const c of chosen) rows.push({ start: c.s.start, type: "item", html: cardHtml(c.item), time: `${hm(c.s.start)}–${hm(c.s.end)}` });

  for (const s of state.sessions) {
    if (s.day === day && s.kind === "logistics" && s.start < s.end && !/meeting/i.test(s.title)) {
      rows.push({ start: s.start, type: "break", html: esc(s.title), time: `${hm(s.start)}–${hm(s.end)}` });
    }
  }

  // Open slots: talk/workshop sessions that don't clash with anything chosen, merged into blocks.
  const chosenS = chosen.map((c) => c.s);
  const open = state.sessions
    .filter((s) => s.day === day && (s.kind === "talk" || s.kind === "workshop") && s.start < s.end && !inPlan(s.id) && !chosenS.some((c) => clashes(c, s)))
    .sort((a, b) => a.start.localeCompare(b.start));
  const blocks = [];
  for (const s of open) {
    const last = blocks[blocks.length - 1];
    if (last && s.start <= last.end) { last.list.push(s); if (s.end > last.end) last.end = s.end; }
    else blocks.push({ start: s.start, end: s.end, list: [s] });
  }
  for (const b of blocks) {
    const list = b.list.sort((x, y) => scoreOf(y.id) - scoreOf(x.id) || x.start.localeCompare(y.start));
    const rel = list.filter((x) => scoreOf(x.id) > 0.3).length;
    rows.push({
      start: b.start, type: "gap", time: `${hm(b.start)}–${hm(b.end)}`,
      html: `<details class="gap"><summary>Open slot · ${list.length} session${list.length > 1 ? "s" : ""} running${rel ? ` · ${rel} relevant` : ""}</summary>
        <div class="gap-list">${list.slice(0, 15).map((o) => altHtml(o)).join("")}</div></details>`,
    });
  }
  const order = { break: 0, item: 1, gap: 2 };
  rows.sort((a, b) => a.start.localeCompare(b.start) || order[a.type] - order[b.type]);
  return rows;
}

function dayHtml(day) {
  const rows = dayRows(day);
  const socials = state.sessions.filter((s) => s.day === day && s.kind === "social" && !inPlan(s.id));
  let html = `<h2 class="day-print-title">${esc(dayLabel(day))}</h2><div class="timeline">`;
  html += rows.map((r) => r.type === "break"
    ? `<div class="trow break"><div class="ttime">${r.time}</div><div>${r.html}</div></div>`
    : `<div class="trow"><div class="ttime">${r.time}</div><div>${r.html}</div></div>`).join("");
  html += `</div>`;
  if (!rows.some((r) => r.type === "item")) html += `<p class="empty">Nothing planned for this day yet. Open a slot above or ask in the chat.</p>`;
  if (socials.length) {
    html += `<h3 class="section-title">Social &amp; networking</h3>${socials.map((s) => altHtml(s)).join("")}`;
  }
  return html;
}

function renderDay() {
  $("#dayView").innerHTML = state.day ? dayHtml(state.day) : `<p class="empty">No days selected.</p>`;
}

function renderBrowse() {
  const q = $("#browseQuery").value.trim().toLowerCase();
  const day = $("#browseDay").value;
  const stream = $("#browseStream").value;
  const terms = q.split(/\s+/).filter(Boolean);
  const list = state.sessions.filter((s) => {
    if (s.kind === "logistics") return false;
    if (day && s.day !== day) return false;
    if (stream && s.stream_code !== stream) return false;
    if (!terms.length) return true;
    const hay = `${s.key} ${s.title} ${s.stream} ${s.abstract} ${s.speakers.map((p) => p.name + " " + p.company).join(" ")}`.toLowerCase();
    return terms.every((t) => hay.includes(t));
  }).sort((a, b) => (terms.length ? scoreOf(b.id) - scoreOf(a.id) : 0) || a.start.localeCompare(b.start));
  $("#browseList").innerHTML = `<p class="small muted">${list.length} sessions</p>` + list.slice(0, 150).map((s) => `
    <div class="browse-item">
      <div class="when">${esc(dayLabel(s.day))}<br>${hm(s.start)}–${hm(s.end)}</div>
      <div><div class="card-top">${sessionMeta(s)}</div>
        <div><button class="link" data-open="${s.id}">${esc(s.title)}</button></div>${speakersHtml(s)}</div>
      <div>${inPlan(s.id) ? `<button class="small" data-remove="${s.id}">Remove</button>` : `<button class="small" data-add="${s.id}">Add</button>`}</div>
    </div>`).join("");
}

function openSession(id) {
  const s = state.byId.get(id);
  if (!s) return;
  const parallel = state.sessions.filter((o) => o.kind !== "logistics" && o.kind !== "social" && clashes(o, s));
  $("#sessionDialogBody").innerHTML = `
    <div class="card-top">${sessionMeta(s)}</div>
    <h2 style="margin:6px 0">${esc(s.title)}</h2>
    <dl class="meta-grid">
      <dt>When</dt><dd>${esc(dayLabel(s.day))}, ${hm(s.start)}–${hm(s.end)} (GMT)</dd>
      <dt>Where</dt><dd>${esc(s.room || "TBC")}</dd>
      <dt>Type</dt><dd>${esc(kindLabel(s.kind))}${s.level ? ` · ${esc(s.level)} level` : ""}</dd>
      ${s.speakers.map((p) => `<dt>Presenter</dt><dd>${esc(p.name)}${p.job_title ? `, ${esc(p.job_title)}` : ""}${p.company ? ` — ${esc(p.company)}` : ""}</dd>`).join("")}
      ${s.tags.length ? `<dt>Tags</dt><dd>${s.tags.map(esc).join(", ")}</dd>` : ""}
    </dl>
    <p class="abstract-text">${esc(s.abstract || "No abstract published.")}</p>
    <div class="card-actions">
      ${inPlan(s.id) ? `<button data-remove="${s.id}" class="danger">Remove from itinerary</button>` : `<button class="primary" data-add="${s.id}">Add to itinerary</button>`}
    </div>
    ${parallel.length ? `<h3 class="section-title">At the same time (${parallel.length})</h3>${parallel.map((o) => altHtml(o, "Swap")).join("")}` : ""}`;
  const dlg = $("#sessionDialog");
  if (!dlg.open) dlg.showModal();
}

// ---------- chat ----------
function mdToHtml(text) {
  // Minimal, safe Markdown: escape first, then bold, bullets and [KEY] links.
  const lines = esc(text).split("\n");
  let html = "", inList = false;
  for (const raw of lines) {
    const m = raw.match(/^\s*(?:[-*•]|\d+\.)\s+(.*)$/);
    if (m) {
      if (!inList) { html += "<ul>"; inList = true; }
      html += `<li>${m[1]}</li>`;
    } else {
      if (inList) { html += "</ul>"; inList = false; }
      if (raw.trim()) html += `<p>${raw}</p>`;
    }
  }
  if (inList) html += "</ul>";
  html = html.replace(/\*\*(.+?)\*\*/g, "<strong>$1</strong>");
  return html.replace(/\[([A-Za-z]{1,4}\d{1,3}|[0-9a-f]{8})\]/g, (all, key) => {
    const s = state.byKey.get(key.toUpperCase()) || state.byKey.get(key.toLowerCase());
    return s ? `<button class="ref" data-open="${s.id}">${esc(key)}</button>` : all;
  });
}

function renderChat(pending = false) {
  const log = $("#chatLog");
  log.innerHTML = state.chatLog.map((m) => `
    <div class="msg ${m.role}${m.error ? " error" : ""}">
      ${m.role === "user" ? `<p>${esc(m.text)}</p>` : mdToHtml(m.text)}
      ${m.actions?.length ? `<div class="actions-list">${m.actions.map((a) => `<span class="action">${esc(a)}</span>`).join("")}</div>` : ""}
    </div>`).join("") + (pending ? `<div class="typing">Thinking…</div>` : "");
  if (!state.chatLog.length && !pending) {
    log.innerHTML = `<div class="msg assistant"><p>Your first itinerary is ready. Ask me to swap sessions, find talks on a topic, explain why something was picked, or suggest something completely different.</p></div>`;
  }
  log.scrollTop = log.scrollHeight;
  const sugg = [
    "Why did you pick these for Monday?",
    "What's on Tuesday at 14:00?",
    "Find me something on Rust or other new languages",
    "Swap advanced sessions for beginner ones",
    "Suggest a few sessions outside my comfort zone",
  ];
  $("#suggestions").innerHTML = state.chatLog.length > 2 ? "" : sugg.map((s) => `<button class="chip" data-suggest="${esc(s)}">${esc(s)}</button>`).join("");
}

async function sendChat(text) {
  text = text.trim();
  if (!text) return;
  state.chatLog.push({ role: "user", text });
  renderChat(true);
  $("#sendBtn").disabled = true;
  $("#chatInput").value = "";
  try {
    const res = await api("/api/chat", { message: text, transcript: state.transcript, profile: state.profile, items: state.items });
    state.transcript = res.transcript;
    const profileChanged = JSON.stringify(res.profile) !== JSON.stringify(state.profile);
    state.profile = res.profile;
    state.items = res.items;
    if (profileChanged) await refreshScores();
    state.chatLog.push({ role: "assistant", text: res.reply, actions: res.actions, error: res.error });
  } catch (e) {
    state.chatLog.push({ role: "assistant", text: `Something went wrong: ${e.message}`, error: true });
  } finally {
    $("#sendBtn").disabled = false;
    save();
    renderChat();
    render();
  }
}

// ---------- calendar export ----------
function icsEscape(s) { return String(s).replace(/\\/g, "\\\\").replace(/;/g, "\\;").replace(/,/g, "\\,").replace(/\r?\n/g, "\\n"); }
function icsFold(line) {
  const out = [];
  let cur = "";
  for (const ch of line) {
    if (new TextEncoder().encode(cur + ch).length > 74) { out.push(cur); cur = " " + ch; } else cur += ch;
  }
  out.push(cur);
  return out.join("\r\n");
}
function exportIcs() {
  const stamp = (iso) => iso.replace(/[-:]/g, "") + "00Z"; // event times are GMT (= UTC in November)
  const now = new Date().toISOString().replace(/[-:]/g, "").slice(0, 15) + "Z";
  const lines = ["BEGIN:VCALENDAR", "VERSION:2.0", "PRODID:-//Connect Planner//EN", "CALSCALE:GREGORIAN", "X-WR-CALNAME:PHUSE EU Connect 2026"];
  for (const it of state.items) {
    const s = state.byId.get(it.id);
    if (!s || !(s.start < s.end)) continue;
    const desc = [s.stream ? `${s.stream} (${s.stream_code})` : kindLabel(s.kind),
      s.speakers.map((p) => p.name + (p.company ? ` (${p.company})` : "")).join(", "), "", s.abstract].join("\n");
    lines.push("BEGIN:VEVENT", `UID:${s.id}@connect-planner`, `DTSTAMP:${now}`, `DTSTART:${stamp(s.start)}`, `DTEND:${stamp(s.end)}`,
      icsFold(`SUMMARY:${icsEscape(`[${s.key}] ${s.title}`)}`), icsFold(`LOCATION:${icsEscape(s.room || "")}`),
      icsFold(`DESCRIPTION:${icsEscape(desc)}`), "END:VEVENT");
  }
  lines.push("END:VCALENDAR");
  const blob = new Blob([lines.join("\r\n")], { type: "text/calendar" });
  const a = Object.assign(document.createElement("a"), { href: URL.createObjectURL(blob), download: "phuse-eu-connect-2026.ics" });
  a.click();
  setTimeout(() => URL.revokeObjectURL(a.href), 1000);
}

// ---------- events ----------
function wire() {
  document.addEventListener("focusin", (e) => { if (e.target.dataset?.field) state.lastField = e.target.dataset.field; });

  $("#streamChips").addEventListener("click", (e) => {
    const b = e.target.closest("[data-stream]");
    if (!b) return;
    const ta = $(FIELDS[state.lastField] || FIELDS.existing);
    const list = splitList(ta.value);
    if (!list.some((x) => x.toLowerCase() === b.dataset.stream.toLowerCase())) list.push(b.dataset.stream);
    ta.value = list.join(", ");
    ta.focus();
  });

  $("#profileForm").addEventListener("submit", async (e) => {
    e.preventDefault();
    const p = readForm();
    if (!p.existing.length && !p.new_interests.length && !p.topics.length && !p.presenters.length) {
      toast("Add at least one interest, topic or presenter.");
      return;
    }
    $("#buildBtn").disabled = true;
    try {
      await buildItinerary(p);
      state.day = null;
      showSetup(false);
      render();
      renderChat();
      toast(`${state.items.length} sessions planned`);
    } catch (err) {
      toast(`Could not build itinerary: ${err.message}`);
    } finally {
      $("#buildBtn").disabled = false;
    }
  });

  $("#editProfileBtn").addEventListener("click", () => showSetup(true));
  $("#resetBtn").addEventListener("click", () => {
    if (!confirm("Clear your interests, itinerary and chat?")) return;
    Object.assign(state, { profile: null, items: [], scores: {}, transcript: [], chatLog: [], day: null });
    save();
    showSetup(true);
  });

  $("#dayTabs").addEventListener("click", (e) => {
    const t = e.target.closest("[data-day]");
    if (!t) return;
    state.day = t.dataset.day;
    save();
    render();
  });

  // Delegated actions anywhere on the page (cards, browse, dialog, chat refs).
  document.addEventListener("click", (e) => {
    const t = e.target.closest("[data-add],[data-remove],[data-open],[data-alts],[data-suggest]");
    if (!t) return;
    if (t.dataset.add) {
      addSession(t.dataset.add);
      if ($("#sessionDialog").open) openSession(t.dataset.add);
    } else if (t.dataset.remove) {
      removeSession(t.dataset.remove);
      if ($("#sessionDialog").open) openSession(t.dataset.remove);
    } else if (t.dataset.open) {
      openSession(t.dataset.open);
    } else if (t.dataset.alts) {
      const box = $(`#alts-${t.dataset.alts}`);
      box.hidden = !box.hidden;
    } else if (t.dataset.suggest) {
      sendChat(t.dataset.suggest);
    }
  });

  $("#browseBtn").addEventListener("click", () => {
    state.browsing = !state.browsing;
    $("#browse").hidden = !state.browsing;
    $("#dayView").hidden = state.browsing;
    $("#dayTabs").style.visibility = state.browsing ? "hidden" : "";
    $("#browseBtn").textContent = state.browsing ? "Back to itinerary" : "Browse all";
    render();
    if (state.browsing) $("#browseQuery").focus();
  });
  ["#browseQuery", "#browseDay", "#browseStream"].forEach((s) => $(s).addEventListener("input", renderBrowse));

  $("#shuffleBtn").addEventListener("click", async () => {
    if (!state.profile) return;
    const p = { ...state.profile, seed: (state.profile.seed || 0) + 1, discovery: Math.max(1, state.profile.discovery ?? 1) };
    await buildItinerary(p);
    render();
    toast("New 'something different' picks");
  });
  $("#icsBtn").addEventListener("click", exportIcs);
  $("#printBtn").addEventListener("click", () => window.print());
  window.addEventListener("beforeprint", () => {
    const days = state.meta.days.filter((d) => !state.profile?.days?.length || state.profile.days.includes(d.date));
    $("#dayView").innerHTML = days.map((d) => dayHtml(d.date)).join("");
  });
  window.addEventListener("afterprint", renderDay);

  $("#chatForm").addEventListener("submit", (e) => { e.preventDefault(); sendChat($("#chatInput").value); });
  $("#chatInput").addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); sendChat($("#chatInput").value); }
  });
  $("#clearChatBtn").addEventListener("click", () => { state.transcript = []; state.chatLog = []; save(); renderChat(); });
}

// ---------- start ----------
async function init() {
  load();
  const [meta, sessions] = await Promise.all([api("/api/meta"), api("/api/sessions")]);
  state.meta = meta;
  state.sessions = sessions;
  for (const s of sessions) { state.byId.set(s.id, s); state.byKey.set(s.key.toUpperCase(), s); state.byKey.set(s.key.toLowerCase(), s); }
  state.items = state.items.filter((i) => state.byId.has(i.id)); // drop sessions no longer on the agenda

  $("#eventLine").innerHTML = `${esc(meta.event)} · Glasgow<span class="fetched"> · agenda fetched ${esc(meta.fetched_at)}</span>`;
  $("#chatMode").textContent = meta.chat
    ? (meta.chat.provider === "Ollama"
        ? `Chatting with a local model (${meta.chat.model} via Ollama). Replies can take a little while; changes apply to your itinerary straight away.`
        : `Chatting with Claude (${meta.chat.model}). Changes apply to your itinerary straight away.`)
    : "No chat model is configured on this server, so the chat understands simple commands: add ET11, remove ET11, details ET11, find shiny, what's on Tue 14:00.";
  $("#streamChips").innerHTML = meta.streams.map((s) => `<button type="button" class="chip" data-stream="${esc(s.name)}" title="${s.count} sessions"><span class="badge" style="--h:${hue(s.code)}">${esc(s.code)}</span> ${esc(s.name)}</button>`).join("");
  $("#dayChecks").innerHTML = meta.days.map((d) => `<label><input type="checkbox" value="${d.date}" checked> ${esc(d.label)}${d.date === "2026-11-15" ? " <span class='muted small'>(workshops)</span>" : ""}</label>`).join("");
  $("#browseDay").innerHTML = `<option value="">All days</option>` + meta.days.map((d) => `<option value="${d.date}">${esc(d.label)}</option>`).join("");
  $("#browseStream").innerHTML = `<option value="">All streams</option>` + meta.streams.map((s) => `<option value="${s.code}">${esc(s.code)} · ${esc(s.name)}</option>`).join("");

  wire();
  if (state.profile && state.items.length) {
    showSetup(false);
    render();
    renderChat();
  } else {
    showSetup(true);
  }
}

init().catch((e) => {
  document.body.insertAdjacentHTML("beforeend", `<p class="empty">Could not load the agenda: ${esc(e.message)}</p>`);
});
