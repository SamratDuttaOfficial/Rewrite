// Download buttons and community builds for the Rewrite site.
// Official installers come from the latest GitHub release; systems without one link to build.html, which lists
// community builds the maintainer has approved (issues labelled "community-build" and "approved").

const REPO = "SamratDuttaOfficial/rewrite";
const FALLBACK_TAG = "v2.0.1";

const PLATFORMS = [
  { id: "win-x64", name: "Windows", sys: "Windows 10 and 11, 64-bit Intel or AMD. Also runs on Windows on ARM.",
    how: "Run it. No admin rights needed.", match: /Windows-Setup\.exe$/, file: "Rewrite-2.0.1-Windows-Setup.exe", os: "windows" },
  { id: "mac-arm64", name: "macOS (Apple Silicon)", sys: "macOS 11 or later on M1, M2, M3, M4 Macs.",
    how: "Drag to Applications. The first time, right-click and choose Open.", match: /macOS-arm64\.dmg$/, file: "Rewrite-2.0.1-macOS-arm64.dmg", os: "mac" },
  { id: "linux-deb", name: "Debian / Ubuntu", sys: "Debian 13, Ubuntu 24.04 and later, Mint, Pop!_OS. 64-bit.",
    how: "sudo apt install ./rewrite_*.deb", match: /_amd64\.deb$/, file: "rewrite_2.0.1_amd64.deb", os: "linux" },
  { id: "linux-appimage", name: "Linux AppImage", sys: "Any 64-bit Intel or AMD distribution: Fedora, Arch, openSUSE.",
    how: "chmod +x the file, then run it.", match: /x86_64\.AppImage$/, file: "Rewrite-2.0.1-x86_64.AppImage", os: "linux" },
  // Not built yet: these go to the build page.
  { id: "mac-x64", name: "macOS (Intel)", sys: "Macs with an Intel processor, macOS 11 or later.", os: "mac",
    target: "macos-x64", setup: "brew install gtk4 pkg-config rustup && rustup-init -y", out: "Rewrite-<version>-macOS-x64.dmg" },
  { id: "linux-arm64", name: "Linux on ARM", sys: "Raspberry Pi 5, ARM laptops and servers, 64-bit (aarch64).", os: "linux",
    target: "linux-arm64", setup: "sudo apt install libgtk-4-dev libfuse2 file build-essential curl\ncurl https://sh.rustup.rs -sSf | sh", out: "rewrite_<version>_arm64.deb, Rewrite-<version>-aarch64.AppImage" },
  { id: "linux-rpm", name: "Fedora / openSUSE (.rpm)", sys: "An .rpm package for Fedora, RHEL and openSUSE. The AppImage works there today.", os: "linux",
    target: "linux-x64", setup: "sudo dnf install gtk4-devel gcc rpm-build curl\ncurl https://sh.rustup.rs -sSf | sh", out: "rewrite-<version>.x86_64.rpm" },
  { id: "win-arm64", name: "Windows on ARM (native)", sys: "Snapdragon laptops. The Windows installer above already runs there under emulation.", os: "windows",
    target: "windows-arm64", setup: "MSYS2 (msys2.org), CLANGARM64 shell:\npacman -S mingw-w64-clang-aarch64-gtk4 mingw-w64-clang-aarch64-pkgconf mingw-w64-clang-aarch64-rust", out: "Rewrite-<version>-Windows-arm64-Setup.exe" },
];

const byId = Object.fromEntries(PLATFORMS.map(p => [p.id, p]));

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
    if (!r.ok) throw new Error(r.status);
    return await r.json();
  } catch {
    return null;
  }
}

// Approved community builds, parsed from the issue form's "### Field" sections.
async function communityBuilds() {
  try {
    const r = await fetch(`https://api.github.com/repos/${REPO}/issues?labels=community-build,approved&state=all&per_page=100`);
    if (!r.ok) throw new Error(r.status);
    const issues = await r.json();
    return issues.map(i => {
      const f = {};
      for (const part of (i.body || "").split(/^### /m).slice(1)) {
        const [head, ...rest] = part.split("\n");
        f[head.trim().toLowerCase()] = rest.join("\n").trim();
      }
      const link = (f["download link"] || "").match(/https:\/\/\S+/);
      const platform = PLATFORMS.find(p => (f["platform"] || "").startsWith(p.name));
      return platform && link ? {
        platform: platform.id, url: link[0], version: f["rewrite version"] || "", sha: f["sha-256"] || "",
        by: i.user && i.user.login, issue: i.html_url,
      } : null;
    }).filter(Boolean);
  } catch {
    return null;
  }
}

function fmtSize(b) { return b > 1e6 ? `${Math.round(b / 1e6)} MB` : ""; }
function esc(s) { return String(s).replace(/[&<>"']/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]); }

async function renderDownloads() {
  const grid = document.getElementById("downloads");
  if (!grid) return;
  const os = guessOS();
  const [rel, cbs] = await Promise.all([latestRelease(), communityBuilds()]);
  const tag = rel ? rel.tag_name : FALLBACK_TAG;
  grid.innerHTML = PLATFORMS.map(p => {
    const asset = p.match && rel && rel.assets.find(a => p.match.test(a.name));
    const ready = !!p.match;
    const url = asset ? asset.browser_download_url
      : ready ? `https://github.com/${REPO}/releases/download/${FALLBACK_TAG}/${p.file}` : `build.html?p=${p.id}`;
    const mine = (cbs || []).filter(c => c.platform === p.id);
    const yours = os && p.os === os && ready;
    let btn;
    if (ready) btn = `<a class="btn ${yours ? "primary" : ""}" href="${url}">Download <small>${esc(asset ? fmtSize(asset.size) : tag)}</small></a>`;
    else if (mine.length) btn = `<a class="btn" href="build.html?p=${p.id}">Community build <small>${mine.length}</small></a>`;
    else btn = `<a class="btn" href="build.html?p=${p.id}">Not built yet: help <small>&rarr;</small></a>`;
    return `<div class="${yours ? "yours" : ""}">
      ${yours ? `<span class="tag">Your system</span>` : !ready ? `<span class="tag">${mine.length ? "Community" : "Needs a build"}</span>` : ""}
      <h3>${esc(p.name)}</h3>
      <div class="sys">${esc(p.sys)}</div>
      ${ready ? `<div class="how">${esc(p.how)}</div>` : ""}
      ${btn}
    </div>`;
  }).join("");
  const v = document.getElementById("version");
  if (v) v.textContent = tag.replace(/^v/, "");
}

async function renderBuildPage() {
  const root = document.getElementById("build");
  if (!root) return;
  const p = byId[new URLSearchParams(location.search).get("p")] || PLATFORMS.find(x => !x.match);
  document.title = `Build Rewrite for ${p.name}`;
  root.querySelector("[data-name]").textContent = p.name;
  root.querySelectorAll("[data-name-inline]").forEach(e => (e.textContent = p.name));
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
    list.innerHTML = `<p>Could not load the list. See <a href="https://github.com/${REPO}/issues?q=label%3Acommunity-build+label%3Aapproved">approved community builds on GitHub</a>.</p>`;
    return;
  }
  const mine = cbs.filter(c => c.platform === p.id);
  list.innerHTML = mine.length
    ? `<div class="list-cb">${mine.map(c => `<div>
        <span>Rewrite ${esc(c.version)} built by <a href="https://github.com/${esc(c.by)}">${esc(c.by)}</a> · <a href="${esc(c.issue)}">details</a></span>
        <a class="btn" href="${esc(c.url)}">Download</a>
        ${c.sha ? `<span class="sha">SHA-256 ${esc(c.sha)}</span>` : ""}
      </div>`).join("")}</div>
      <p class="note">Community builds are made by other people, not by the developer. Check the SHA-256 after downloading.</p>`
    : `<p>No one has submitted a build for ${esc(p.name)} yet. Yours could be the first.</p>`;
}

renderDownloads();
renderBuildPage();
