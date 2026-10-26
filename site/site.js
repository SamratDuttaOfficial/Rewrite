// The Rewrite site: download list, community builds, the editor example and the document tabs.
// Official installers come from the latest GitHub release. Systems without one link to build.html, which lists
// community builds the maintainer has approved (issues labelled "community-build" and "approved").

const REPO = "SamratDuttaOfficial/Rewrite";
const FALLBACK_TAG = "v2.0.1";

// An official row whose file is not in the latest release is shown as a build row, so no button leads to a
// missing file. Rows without "match" are only ever built by the community.
const PLATFORMS = [
  { id: "win-x64", name: "Windows", sys: "Windows 10, 11 · Intel, AMD", match: /Windows-Setup\.exe$/, os: "windows", cpu: "x86" },
  { id: "win-arm64", name: "Windows on ARM", sys: "Windows 11 · Snapdragon", match: /Windows-arm64-Setup\.exe$/, os: "windows", cpu: "arm" },
  { id: "mac-arm64", name: "macOS, Apple Silicon", sys: "macOS 15+ · M-series", match: /macOS-arm64\.dmg$/, os: "mac", cpu: "arm" },
  { id: "mac-x64", name: "macOS, Intel", sys: "macOS 15+ · Intel", match: /macOS-x86_64\.dmg$/, os: "mac", cpu: "x86" },
  { id: "linux-deb", name: "Debian / Ubuntu", sys: "Ubuntu 24.04+, Debian 13+", match: /_amd64\.deb$/, os: "linux", cpu: "x86" },
  { id: "linux-deb-arm64", name: "Debian / Ubuntu, ARM", sys: "Raspberry Pi 5, ARM laptops", match: /_arm64\.deb$/, os: "linux", cpu: "arm" },
  { id: "linux-appimage", name: "Linux AppImage", sys: "Any distribution · x86_64", match: /x86_64\.AppImage$/, os: "linux", cpu: "x86" },
  { id: "linux-appimage-arm64", name: "Linux AppImage, ARM", sys: "Any distribution · aarch64", match: /aarch64\.AppImage$/, os: "linux", cpu: "arm" },
  { id: "linux-rpm", name: "Fedora / openSUSE .rpm", sys: "The AppImage works there today", os: "linux",
    setup: "sudo dnf install gtk4-devel gcc rpm-build curl\ncurl https://sh.rustup.rs -sSf | sh", out: "rewrite-<version>.x86_64.rpm" },
];
const byId = Object.fromEntries(PLATFORMS.map(p => [p.id, p]));

const icon = name => `<svg class="ic"><use href="icons.svg#${name}"/></svg>`;

function esc(s) { return String(s).replace(/[&<>"']/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]); }
function fmtSize(b) { return b ? `${Math.round(b / 1e6)} MB` : ""; }

function guessOS() {
  const ua = navigator.userAgent;
  if (/Windows/.test(ua)) return "windows";
  if (/Macintosh|Mac OS X/.test(ua) && !/iPhone|iPad/.test(ua)) return "mac";
  if (/Linux|X11/.test(ua) && !/Android/.test(ua)) return "linux";
  return "";
}

// "arm" or "x86" where the browser says (Chromium's client hints, or the user agent on Linux), otherwise the
// usual one: x86 on Windows and Linux, Apple Silicon on a Mac
async function guessCPU(os) {
  try {
    if (navigator.userAgentData && navigator.userAgentData.getHighEntropyValues) {
      const v = await navigator.userAgentData.getHighEntropyValues(["architecture"]);
      if (v.architecture) return v.architecture === "arm" ? "arm" : "x86";
    }
  } catch {}
  if (/aarch64|arm64|armv8/i.test(navigator.userAgent)) return "arm";
  return os === "mac" ? "arm" : "x86";
}

async function latestRelease() {
  try {
    const r = await fetch(`https://api.github.com/repos/${REPO}/releases/latest`);
    return r.ok ? await r.json() : null;
  } catch { return null; }
}

// Approved community builds, parsed from the issue form's "### Field" sections.
async function communityBuilds() {
  try {
    const r = await fetch(`https://api.github.com/repos/${REPO}/issues?labels=community-build,approved&state=all&per_page=100`);
    if (!r.ok) return null;
    return (await r.json()).map(i => {
      const f = {};
      for (const part of (i.body || "").split(/^### /m).slice(1)) {
        const [head, ...rest] = part.split("\n");
        f[head.trim().toLowerCase()] = rest.join("\n").trim();
      }
      const link = (f["download link"] || "").match(/https:\/\/\S+/);
      const p = PLATFORMS.find(x => (f["platform"] || "") === x.name);
      return p && link ? { platform: p.id, url: link[0], version: f["rewrite version"] || "", sha: f["sha-256"] || "", by: i.user && i.user.login, issue: i.html_url } : null;
    }).filter(Boolean);
  } catch { return null; }
}

async function renderDownloads() {
  const list = document.getElementById("downloads");
  if (!list) return;
  const os = guessOS();
  const [cpu, rel, cbs] = await Promise.all([guessCPU(os), latestRelease(), communityBuilds()]);
  const tag = rel ? rel.tag_name : FALLBACK_TAG;
  document.getElementById("version").textContent = tag.replace(/^v/, "");
  const assetOf = p => p.match && rel && rel.assets.find(x => p.match.test(x.name));
  const ready = PLATFORMS.filter(p => assetOf(p));
  const mine = ready.find(p => p.os === os && p.cpu === cpu) || ready.find(p => p.os === os);
  if (mine) {
    document.getElementById("os-name").textContent = mine.name;
    document.querySelector(".side .btn").href = assetOf(mine).browser_download_url;
  }
  const rows = PLATFORMS.map(p => {
    const a = assetOf(p);
    if (a) {
      const me = p === mine;
      return `<div class="row${me ? " mine" : ""}"><span class="ico">${icon("download")}</span>
        <span class="fname">${esc(a.name)}</span>
        <span class="sys"><b>${esc(p.name)}</b>${esc(p.sys)}</span>
        <span class="size">${fmtSize(a.size)}</span>
        <span class="act"><a class="btn${me ? " blue" : ""}" href="${a.browser_download_url}">${icon("download")}Download</a></span></div>`;
    }
    const n = (cbs || []).filter(c => c.platform === p.id).length;
    return `<div class="row todo"><span class="ico">${icon("build")}</span>
      <span class="fname">${n ? `${n} community build${n > 1 ? "s" : ""}` : "Not built yet"}</span>
      <span class="sys"><b>${esc(p.name)}</b>${esc(p.sys)}</span>
      <span class="size"></span>
      <span class="act"><a class="btn" href="build.html?p=${p.id}">${icon(n ? "next" : "build")}${n ? "See builds" : "Build it"}</a></span></div>`;
  });
  list.innerHTML = `<div class="hdr"><span></span><span>File</span><span>System</span><span style="text-align:right">Size</span><span></span></div>` + rows.join("");
}

// -- the editor example: every suggestion below is real output from Rewrite 2.0.1 (the auto-rewrite of this text
// with the spelling slip fixed, Medium strength, all three kinds of change) ---------------------------------------

const DOC = [
  ["In the realm of urban planning, trees are often overlooked ", { k: "wordy", id: "p1", t: "due to the fact that" }, " they take years to grow."],
  [{ k: "sent", id: "s2", t: ["Trees are ", { k: "word", id: "w1", t: "cheap" }, " to plant and keep streets cool."] },
    " Even a few trees can make ", { k: "spell", id: "sp", t: "evrey" }, " street feel calmer."],
];
const SUG = {
  p1: { head: "Or reword the whole phrase", what: "due to the fact that", alts: [["because", 94, "phrase", "meaning 97%"], ["as", 11, "phrase", "meaning 98%"]] },
  w1: { head: "Alternatives for", what: "cheap", alts: [["inexpensive", 83, "synonym", "relatively low in price or charging low prices"], ["affordable", 43, "related"], ["low-cost", 29, "related"]] },
  sp: { head: "Spelling", what: "evrey", alts: [["every", 77, "spelling"], ["even", 29, "spelling"], ["ever", 14, "spelling"]] },
  s2: { head: "Whole sentence", what: "", cards: [
    ["Planting trees is a cost-effective solution for keeping the streets cool.", "meaning 97% · 73% reworded"],
    ["Trees are a cost-effective solution for keeping the streets cool.", "meaning 98% · 60% reworded"],
    ["The cost of planting trees is low, and they are a practical solution for keeping the streets cool.", "meaning 94% · 78% reworded"]] },
};
const AUTO = [
  "Trees are often neglected in urban planning because it takes years for them to grow.",
  "Keeping streets cool is possible with inexpensive trees. Every street can be made more peaceful with the addition of a few trees.",
];
const HINT = `<div class="head">Try it</div><p class="hint" style="margin-top:6px">
  Click the <b>highlight</b>, the word <b>cheap</b>, the <b>squiggle</b> or the <b>second sentence</b>.
  Or press <b>Auto-rewrite</b>.</p>`;

let state;
function resetDemo() { state = { text: {}, changed: new Set(), auto: false }; drawDoc(); showPanel(null); }

function piece(x) {
  if (typeof x === "string") return esc(x);
  const inner = Array.isArray(x.t) ? x.t.map(piece).join("") : esc(state.text[x.id] ?? x.t);
  const cls = state.changed.has(x.id) ? "changed" : (state.text[x.id] && x.k !== "sent" ? "" : x.k);
  return `<span class="${cls}" data-id="${x.id}" ${state.changed.has(x.id) ? "" : 'role="button" tabindex="0"'}>${inner}</span>`;
}
function drawDoc() {
  const doc = document.getElementById("doc");
  if (state.auto) {
    doc.innerHTML = AUTO.map(t => `<p><span class="changed">${esc(t)}</span></p>`).join("");
    return;
  }
  doc.innerHTML = DOC.map(p => `<p>${p.map(piece).join("")}</p>`).join("");
}
function showPanel(id) {
  const panel = document.getElementById("panel");
  document.querySelectorAll("#doc .on").forEach(e => e.classList.remove("on"));
  if (!id) { panel.innerHTML = HINT; return; }
  const el = document.querySelector(`#doc [data-id="${id}"]`);
  if (el && id !== "s2") el.classList.add("on");
  const s = SUG[id];
  let html = `<div class="head">${s.head}</div>${s.what ? `<div class="what">${esc(s.what)}</div>` : '<div style="height:8px"></div>'}`;
  if (s.alts) html += s.alts.map(([w, fit, tag, gl], i) => `<button class="alt" data-pick="${id}" data-w="${esc(w)}" type="button">
      <span class="n">${i + 1}</span><span class="w">${esc(w)}</span><span class="fit"><i style="width:${fit}%"></i></span>
      <span class="tag${tag === "related" ? " rel" : ""}">${tag}</span>${gl ? `<span class="gl">${esc(gl)}</span>` : ""}</button>`).join("");
  if (s.cards) html += s.cards.map(([t, m]) => `<button class="card" data-pick="${id}" data-w="${esc(t)}" type="button">${esc(t)}<small>${m}</small></button>`).join("");
  panel.innerHTML = html;
}
function pick(id, w) {
  if (id === "s2") {
    // the whole sentence: replace the sentence span's contents with plain text
    DOC[1][0] = { k: "sent", id: "s2", t: w };
  } else state.text[id] = w;
  state.changed.add(id);
  document.getElementById("reset").hidden = false;
  drawDoc();
  showPanel(null);
}
function initDemo() {
  const demo = document.getElementById("demo");
  if (!demo) return;
  const original = JSON.stringify(DOC[1][0]);
  resetDemo();
  demo.addEventListener("click", e => {
    const p = e.target.closest("[data-pick]");
    if (p) return pick(p.dataset.pick, p.dataset.w);
    const t = e.target.closest("#doc [data-id]");
    if (!t || state.changed.has(t.dataset.id) || state.auto) return;
    const inner = e.target.closest("#doc [data-id]:not([data-id=s2])");
    showPanel(inner ? inner.dataset.id : t.dataset.id);
  });
  demo.addEventListener("keydown", e => { if ((e.key === "Enter" || e.key === " ") && e.target.matches("#doc [data-id]")) { e.preventDefault(); e.target.click(); } });
  document.getElementById("rw").addEventListener("click", () => {
    state.auto = true; drawDoc();
    document.getElementById("panel").innerHTML = `<div class="head">Auto-rewrite</div><p class="hint" style="margin-top:6px">
      Done. In the app, accept or reject each change.</p>`;
    document.getElementById("reset").hidden = false;
  });
  document.getElementById("reset").addEventListener("click", () => {
    DOC[1][0] = JSON.parse(original); document.getElementById("reset").hidden = true; resetDemo();
  });
}

function initTheme() {
  const btn = document.getElementById("theme");
  if (!btn) return;
  const root = document.documentElement;
  const dark = () => (root.dataset.theme ? root.dataset.theme === "dark" : matchMedia("(prefers-color-scheme: dark)").matches);
  const show = () => btn.querySelector("use").setAttribute("href", `icons.svg#${dark() ? "sun" : "moon"}`);
  show();
  btn.addEventListener("click", () => {
    root.dataset.theme = dark() ? "light" : "dark";
    try { localStorage.setItem("theme", root.dataset.theme); } catch {}
    show();
  });
}

function initTabs() {
  document.querySelectorAll(".tabs button").forEach(b => b.addEventListener("click", () => {
    document.querySelectorAll(".tabs button").forEach(x => x.setAttribute("aria-selected", x === b));
    document.querySelectorAll(".tabpane").forEach(p => p.classList.toggle("show", p.id === b.dataset.tab));
  }));
}

async function renderBuildPage() {
  const root = document.getElementById("build");
  if (!root) return;
  const p = byId[new URLSearchParams(location.search).get("p")] || PLATFORMS.find(x => !x.match);
  document.title = `Build Rewrite for ${p.name}`;
  root.querySelectorAll("[data-name]").forEach(e => (e.textContent = p.name));
  root.querySelector("[data-sys]").textContent = p.sys;
  root.querySelector("[data-setup]").textContent = p.setup || "";
  root.querySelector("[data-out]").textContent = p.out || "";
  const submit = new URL(`https://github.com/${REPO}/issues/new`);
  submit.searchParams.set("template", "community-build.yml");
  submit.searchParams.set("title", `Community build: ${p.name}`);
  submit.searchParams.set("platform", p.name);
  root.querySelector("[data-submit]").href = submit.href;

  const list = root.querySelector("[data-list]");
  const cbs = await communityBuilds();
  if (cbs === null) {
    list.innerHTML = `<p>The list could not be loaded. See the <a href="https://github.com/${REPO}/issues?q=label%3Acommunity-build+label%3Aapproved">approved builds on GitHub</a>.</p>`;
    return;
  }
  const mine = cbs.filter(c => c.platform === p.id);
  list.innerHTML = mine.length
    ? `<div class="cblist">${mine.map(c => `<div>
        <span>Rewrite ${esc(c.version)}, built by <a href="https://github.com/${esc(c.by)}">${esc(c.by)}</a> (<a href="${esc(c.issue)}">how it was built</a>)</span>
        <a class="btn blue" href="${esc(c.url)}">${icon("download")}Download</a>
        ${c.sha ? `<span class="sha">SHA-256 ${esc(c.sha)}</span>` : ""}</div>`).join("")}</div>
      <p class="warn">Made by other people. Check the SHA-256.</p>`
    : `<p class="sub">None yet.</p>`;
}

renderDownloads();
initDemo();
initTheme();
initTabs();
renderBuildPage();
