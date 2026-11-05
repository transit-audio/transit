(function () {
  var REL = "https://github.com/transit-audio/transit/releases/latest";
  var meta = document.getElementById("release-meta");
  var list = document.getElementById("assets");
  function has(n, word) { return n.toLowerCase().indexOf(word) !== -1; }
  function ends(n, ext) { return n.toLowerCase().slice(-ext.length) === ext; }
  // Each test uses the platform word in the name, never the extension alone.
  var PLATFORMS = [
    ["Windows installer (.msi)", function (n) { return has(n, "windows") && ends(n, ".msi"); }],
    ["Windows portable (.zip)", function (n) { return has(n, "windows") && ends(n, ".zip"); }],
    ["macOS, Apple silicon (.dmg)", function (n) { return has(n, "macos") && ends(n, ".dmg"); }],
    ["macOS portable (.tar.gz)", function (n) { return has(n, "macos") && ends(n, ".tar.gz"); }],
    ["Linux AppImage", function (n) { return has(n, "linux") && ends(n, ".appimage"); }],
    ["Linux .deb (Ubuntu, Debian)", function (n) { return has(n, "linux") && ends(n, ".deb"); }],
    ["Linux .rpm (Fedora)", function (n) { return has(n, "linux") && ends(n, ".rpm"); }],
    ["Linux tar.gz (everything else)", function (n) { return has(n, "linux") && ends(n, ".tar.gz"); }]
  ];
  function row(label, href, note) {
    var li = document.createElement("li");
    var a = document.createElement("a");
    a.href = href;
    a.textContent = label;
    li.appendChild(a);
    if (note) {
      var s = document.createElement("span");
      s.className = "muted";
      s.textContent = note;
      li.appendChild(s);
    }
    list.appendChild(li);
  }
  function fallback() {
    meta.textContent = "Grab the newest build from GitHub.";
    list.textContent = "";
    PLATFORMS.forEach(function (p) { row(p[0], REL); });
  }
  fetch("https://api.github.com/repos/transit-audio/transit/releases/latest", { headers: { Accept: "application/vnd.github+json" } })
    .then(function (r) { if (!r.ok) throw new Error(r.status); return r.json(); })
    .then(function (rel) {
      var date = rel.published_at ? new Date(rel.published_at).toLocaleDateString(undefined, { year: "numeric", month: "long", day: "numeric" }) : "";
      meta.textContent = (rel.name || rel.tag_name) + (date ? ", released " + date : "");
      var assets = rel.assets || [], found = 0;
      PLATFORMS.forEach(function (p) {
        var a = assets.filter(function (x) { return p[1](x.name); })[0];
        if (a) {
          found++;
          row(p[0], a.browser_download_url, a.name + " (" + (a.size / 1048576).toFixed(1) + " MB)");
        } else {
          row(p[0], REL, "see the release page");
        }
      });
      if (!found) fallback();
    })
    .catch(fallback);
})();
