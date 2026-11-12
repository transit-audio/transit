(function () {
  // The releases list: GitHub's "latest" skips pre-releases (tags with a suffix), so the newest
  // entry is resolved from the list instead.
  var REL = "https://github.com/transit-audio/transit/releases";
  var meta = document.getElementById("release-meta");
  function has(n, word) { return n.toLowerCase().indexOf(word) !== -1; }
  function ends(n, ext) { return n.toLowerCase().slice(-ext.length) === ext; }
  // Each test uses the platform word in the name, never the extension alone.
  // The first entry per platform is the primary download, the rest are "Other formats".
  var PLATFORMS = {
    windows: [
      ["installer", function (n) { return has(n, "windows") && ends(n, ".msi"); }],
      ["zip", function (n) { return has(n, "windows") && ends(n, ".zip"); }]
    ],
    mac: [
      ["DMG", function (n) { return has(n, "macos") && ends(n, ".dmg"); }],
      ["tar.gz", function (n) { return has(n, "macos") && ends(n, ".tar.gz"); }]
    ],
    linux: [
      ["AppImage", function (n) { return has(n, "linux") && ends(n, ".appimage"); }],
      [".deb", function (n) { return has(n, "linux") && ends(n, ".deb"); }],
      [".rpm", function (n) { return has(n, "linux") && ends(n, ".rpm"); }],
      ["tar.gz", function (n) { return has(n, "linux") && ends(n, ".tar.gz"); }]
    ]
  };
  // Platform detection: phones and tablets get no tag; otherwise Windows, then Mac, then Linux.
  function detect() {
    var ua = navigator.userAgent || "";
    var pf = (navigator.userAgentData && navigator.userAgentData.platform) || navigator.platform || "";
    if (/Android|iPhone|iPad|iPod|CrOS/i.test(pf + " " + ua)) return "";
    if (/Win/i.test(pf) || (!pf && /Windows/i.test(ua))) return "windows";
    if (/Mac/i.test(pf) || (!pf && /Macintosh/i.test(ua))) return "mac";
    if (/Linux|X11/i.test(pf) || (!pf && /Linux|X11/i.test(ua))) return "linux";
    return "";
  }
  function tagMine() {
    var os = detect();
    var card = os && document.getElementById("dl-" + os);
    if (!card) return;
    card.className += " mine";
    card.querySelector(".tag").hidden = false;
  }
  function link(label, href) {
    var a = document.createElement("a");
    a.href = href;
    a.textContent = label;
    return a;
  }
  function fill(os, assets) {
    var card = document.getElementById("dl-" + os);
    var btn = card.querySelector(".dlbtn");
    var others = card.querySelector(".others");
    var found = PLATFORMS[os].map(function (p) {
      var a = assets.filter(function (x) { return p[1](x.name); })[0];
      return a ? { label: p[0], a: a } : null;
    });
    if (found[0]) {
      btn.href = found[0].a.browser_download_url;
      btn.querySelector(".size").textContent = found[0].a.name + " (" + (found[0].a.size / 1048576).toFixed(1) + " MB)";
    } else {
      btn.href = REL;
    }
    others.textContent = "";
    var n = 0;
    found.slice(1).forEach(function (f) {
      if (!f) return;
      if (n++) others.appendChild(document.createTextNode(", "));
      others.appendChild(link(f.label, f.a.browser_download_url));
    });
    if (!n) others.appendChild(link("see the release page", REL));
  }
  function fallback() {
    meta.textContent = "Grab the newest build from GitHub.";
    Object.keys(PLATFORMS).forEach(function (os) { fill(os, []); });
  }
  tagMine();
  fetch("https://api.github.com/repos/transit-audio/transit/releases?per_page=10", { headers: { Accept: "application/vnd.github+json" } })
    .then(function (r) { if (!r.ok) throw new Error(r.status); return r.json(); })
    .then(function (list) {
      // The newest release that is not a draft, pre-releases included.
      var rel = (Array.isArray(list) ? list : []).filter(function (x) { return x && !x.draft; })[0];
      if (!rel) { fallback(); return; }
      if (rel.html_url) REL = rel.html_url;
      var date = rel.published_at ? new Date(rel.published_at).toLocaleDateString(undefined, { year: "numeric", month: "long", day: "numeric" }) : "";
      meta.textContent = (rel.name || rel.tag_name) + (date ? ", released " + date : "");
      var assets = rel.assets || [];
      if (!assets.length) { fallback(); return; }
      Object.keys(PLATFORMS).forEach(function (os) { fill(os, assets); });
    })
    .catch(fallback);
})();

// Screenshot lightbox: a native <dialog> opened with showModal(), no libraries. Without JavaScript
// each screenshot is a plain link to the full image. Deep link: #shot=2 opens the second one.
(function () {
  var links = Array.prototype.slice.call(document.querySelectorAll("#screenshots a.shot"));
  if (!links.length || typeof HTMLDialogElement === "undefined") return;
  var dlg = document.createElement("dialog");
  dlg.className = "lb";
  dlg.setAttribute("aria-label", "Screenshot viewer");
  dlg.innerHTML =
    '<figure class="lb-fig"><img alt=""><figcaption></figcaption></figure>' +
    '<button type="button" class="lb-btn lb-close" aria-label="Close">Close</button>' +
    '<button type="button" class="lb-btn lb-prev" aria-label="Previous screenshot">\u2039</button>' +
    '<button type="button" class="lb-btn lb-next" aria-label="Next screenshot">\u203a</button>';
  document.body.appendChild(dlg);
  var img = dlg.querySelector("img");
  var cap = dlg.querySelector("figcaption");
  var cur = 0, opener = null;
  function show(i) {
    cur = (i + links.length) % links.length;
    var a = links[cur], src = a.querySelector("img");
    img.src = a.getAttribute("href");
    img.alt = src ? src.alt : "";
    var fc = a.parentNode.querySelector("figcaption");
    cap.textContent = fc ? fc.textContent : "";
  }
  function open(i, from) {
    opener = from || links[i];
    show(i);
    if (!dlg.open) dlg.showModal();
    document.documentElement.className += " lb-open";
  }
  links.forEach(function (a, i) {
    a.addEventListener("click", function (e) {
      if (e.ctrlKey || e.metaKey || e.shiftKey || e.altKey || e.button) return;
      e.preventDefault();
      open(i, a);
    });
  });
  dlg.addEventListener("close", function () {
    document.documentElement.className = document.documentElement.className.replace(/\s*\blb-open\b/, "");
    img.removeAttribute("src");
    if (opener) opener.focus();
  });
  dlg.addEventListener("click", function (e) {
    var t = e.target;
    if (t.closest(".lb-close")) dlg.close();
    else if (t.closest(".lb-prev")) show(cur - 1);
    else if (t.closest(".lb-next")) show(cur + 1);
    else if (t === dlg || t.closest(".lb-fig") === null) dlg.close();
  });
  dlg.addEventListener("keydown", function (e) {
    if (e.key === "ArrowLeft") { e.preventDefault(); show(cur - 1); }
    else if (e.key === "ArrowRight") { e.preventDefault(); show(cur + 1); }
  });
  function fromHash() {
    var m = /^#shot=(\d+)$/.exec(location.hash);
    if (m && +m[1] >= 1 && +m[1] <= links.length) open(+m[1] - 1);
  }
  fromHash();
  window.addEventListener("hashchange", fromHash);
})();
