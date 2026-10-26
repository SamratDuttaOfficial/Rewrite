// The Rewrite site: download list, community builds, the editor example and the document tabs.
// Official installers come from the latest GitHub release. Systems without one link to build.html, which lists
// community builds the maintainer has approved (issues labelled "community-build" and "approved").

const REPO = "SamratDuttaOfficial/rewrite";
const FALLBACK_TAG = "v2.0.1";

const PLATFORMS = [
  { id: "win-x64", name: "Windows", sys: "Windows 10 and 11, 64-bit. Also runs on Windows on ARM.", match: /Windows-Setup\.exe$/, file: "Rewrite-2.0.1-Windows-Setup.exe", os: "windows" },
  { id: "mac-arm64", name: "macOS, Apple Silicon", sys: "macOS 11 or later on M-series Macs.", match: /macOS-arm64\.dmg$/, file: "Rewrite-2.0.1-macOS-arm64.dmg", os: "mac" },
  { id: "linux-deb", name: "Debian / Ubuntu", sys: "Debian 13, Ubuntu 24.04 or later, 64-bit.", match: /_amd64\.deb$/, file: "rewrite_2.0.1_amd64.deb", os: "linux" },
  { id: "linux-appimage", name: "Linux AppImage", sys: "Fedora, Arch, openSUSE and others, 64-bit.", match: /x86_64\.AppImage$/, file: "Rewrite-2.0.1-x86_64.AppImage", os: "linux" },
  // Not built yet: these go to the build page.
  { id: "mac-x64", name: "macOS, Intel", sys: "Macs with an Intel processor.", os: "mac",
    setup: "brew install gtk4 pkg-config rustup\nrustup-init -y", out: "Rewrite-<version>-macOS-x64.dmg" },
  { id: "linux-arm64", name: "Linux on ARM", sys: "Raspberry Pi 5, ARM laptops, aarch64.", os: "linux",
    setup: "sudo apt install libgtk-4-dev libfuse2 file build-essential curl\ncurl https://sh.rustup.rs -sSf | sh", out: "rewrite_<version>_arm64.deb, Rewrite-<version>-aarch64.AppImage" },
  { id: "linux-rpm", name: "Fedora / openSUSE .rpm", sys: "A native .rpm. The AppImage works there today.", os: "linux",
    setup: "sudo dnf install gtk4-devel gcc rpm-build curl\ncurl https://sh.rustup.rs -sSf | sh", out: "rewrite-<version>.x86_64.rpm" },
  { id: "win-arm64", name: "Windows on ARM, native", sys: "Snapdragon laptops. The Windows installer already runs there.", os: "windows",
    setup: "Install MSYS2 from msys2.org, open the CLANGARM64 shell, then:\npacman -S mingw-w64-clang-aarch64-gtk4 mingw-w64-clang-aarch64-pkgconf mingw-w64-clang-aarch64-rust", out: "Rewrite-<version>-Windows-arm64-Setup.exe" },
];
const byId = Object.fromEntries(PLATFORMS.map(p => [p.id, p]));

const ICON = {
  file: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round"><path d="M6 3h8l4 4v14H6z"/><path d="M14 3v4h4"/><path d="M12 10v7m-3-3 3 3 3-3"/></svg>',
  build: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round" stroke-linecap="round"><path d="M6 3h8l4 4v14H6z" stroke-dasharray="2.5 2.5"/><path d="M9 14h6M12 11v6"/></svg>',
};

function esc(s) { return String(s).replace(/[&<>"']/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]); }
function fmtSize(b) { return b ? `${Math.round(b / 1e6)} MB` : ""; }

function guessOS() {
  const ua = navigator.userAgent;
  if (/Windows/.test(ua)) return "windows";
  if (/Macintosh|Mac OS X/.test(ua) && !/iPhone|iPad/.test(ua)) return "mac";
  if (/Linux|X11/.test(ua) && !/Android/.test(ua)) return "linux";
  return "";
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
  const osName = { windows: "Windows", mac: "macOS", linux: "Linux" }[os];
  if (osName) document.getElementById("os-name").textContent = osName;
  const [rel, cbs] = await Promise.all([latestRelease(), communityBuilds()]);
  const tag = rel ? rel.tag_name : FALLBACK_TAG;
  document.getElementById("version").textContent = tag.replace(/^v/, "");
  let marked = false;
  const rows = PLATFORMS.map(p => {
    if (p.match) {
      const a = rel && rel.assets.find(x => p.match.test(x.name));
      const url = a ? a.browser_download_url : `https://github.com/${REPO}/releases/download/${FALLBACK_TAG}/${p.file}`;
      const mine = !marked && os && p.os === os;
      if (mine) marked = true;
      return `<div class="row${mine ? " mine" : ""}"><span class="ico">${ICON.file}</span>
        <span class="fname">${esc(a ? a.name : p.file)}</span>
        <span class="sys"><b>${esc(p.name)}</b>${esc(p.sys)}${mine ? '<span class="yours">Matches this computer</span>' : ""}</span>
        <span class="size">${fmtSize(a && a.size)}</span>
        <span class="act"><a class="btn${mine ? " blue" : ""}" href="${url}">Download</a></span></div>`;
    }
    const n = (cbs || []).filter(c => c.platform === p.id).length;
    return `<div class="row todo"><span class="ico">${ICON.build}</span>
      <span class="fname">${n ? `${n} community build${n > 1 ? "s" : ""}` : "Not built yet"}</span>
      <span class="sys"><b>${esc(p.name)}</b>${esc(p.sys)}</span>
      <span class="size"></span>
      <span class="act"><a class="btn" href="build.html?p=${p.id}">${n ? "See builds" : "Build it"}</a></span></div>`;
  });
  list.innerHTML = `<div class="hdr"><span></span><span>File</span><span>System</span><span style="text-align:right">Size</span><span></span></div>` + rows.join("");
}

// -- the editor example: every suggestion below is real output from Rewrite 2.0.1 ---------------------------

const DOC = [
  ["In today's fast-paced digital landscape, ", { k: "wordy", id: "p1", t: "it is important to note that" }, " many people rely on cloud services for everyday writing."],
  [{ k: "sent", id: "s2", t: ["It is ", { k: "word", id: "w1", t: "fast" }, ", it keeps your drafts private, and it works without an internet connection."] },
    " Small models make mistakes, so you should always read ", { k: "spell", id: "sp", t: "evrey" }, " result carefully."],
];
const SUG = {
  p1: { head: "Or reword the whole phrase", what: "it is important to note that", alts: [["note that", 92, "phrase", "meaning 99%"], ["it is crucial to note that", 85, "synonym"], ["it is critical to note that", 85, "synonym", "meaning 100%"]] },
  w1: { head: "Alternatives for", what: "fast", alts: [["quick", 80, "synonym", "hurried and brief"], ["speedy", 70, "related", "acting or moving quickly"], ["rapid", 40, "related"], ["quicker", 30, "synonym"]] },
  sp: { head: "Spelling", what: "evrey", alts: [["every", 95, "spelling"]] },
  s2: { head: "Whole sentence", what: "", cards: [
    ["Despite being offline and speedy, it ensures your drafts are confidential.", "meaning 93% · 73% reworded"],
    ["It is speedy, it ensures your drafts are confidential, and operates offline.", "meaning 95% · 50% reworded"],
    ["The service is speedy, safeguarding your drafts and operates offline.", "meaning 90% · 60% reworded"]] },
};
const AUTO = [
  "The fast-paced digital world places a significant amount of writing on cloud services, which is a must for many people.",
  "Despite being offline and speedy, it ensures your drafts are confidential.",
];
const HINT = `<div class="head">Get started</div><p class="hint" style="margin-top:6px">
  Click the <b>highlighted phrase</b> to reword it, the dotted word <b>fast</b> for alternatives, the red
  squiggle to fix the spelling, or the <b>second sentence</b> to rephrase all of it.<br><br>
  Or press <b>Auto-rewrite</b> to go through the whole text.</p>`;

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
    doc.innerHTML = `<p><span class="changed">${esc(AUTO[0])}</span></p><p><span class="changed">${esc(AUTO[1])}</span> Small models make mistakes, so you should always read every result carefully.</p>`;
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
      Two sentences rewritten and the spelling fixed. In the app you can now step through each change and accept or
      reject it.</p>`;
    document.getElementById("reset").hidden = false;
  });
  document.getElementById("reset").addEventListener("click", () => {
    DOC[1][0] = JSON.parse(original); document.getElementById("reset").hidden = true; resetDemo();
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
        <a class="btn blue" href="${esc(c.url)}">Download</a>
        ${c.sha ? `<span class="sha">SHA-256 ${esc(c.sha)}</span>` : ""}</div>`).join("")}</div>
      <p class="warn">Community builds are made by other people, not by the developer. Check the SHA-256 after downloading.</p>`
    : `<p>Nobody has sent in a build for ${esc(p.name)} yet.</p>`;
}

renderDownloads();
initDemo();
initTabs();
renderBuildPage();
