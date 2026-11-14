The website, https://transit-audio.com/, built with [Hugo](https://gohugo.io/) and deployed to GitHub
Pages by `.github/workflows/pages.yml`, which pins the Hugo version (0.170.0) and builds this folder at
deploy time. Nothing generated is committed.

- `layouts/home.html` is the home page (with `static/style.css` and `static/app.js`).
- The other pages are built from the markdown in the repository, which stays where it is and is
  mounted into the site (`hugo.toml`, `module.mounts`): `/features/` from the `## Features` section of
  the top-level `README.md`, `/guide/`, `/shortcuts/` and `/troubleshooting/` from the files of the
  same names in `docs/`, and `/releases/` from `docs/releases/vX.Y.Z.md` (a file whose first heading
  says "(in progress)" is shown as not released yet).
- `data/coming.toml` is the reviewed list of planned items shown as "Coming" on the features page;
  `data/nav.toml` is the page list in the header and the home page footer.
- Links between markdown files go to the page here when there is one and to the file on GitHub
  otherwise (`layouts/_markup/render-link.html`). The build fails on a link to a file that does not
  exist, on an unknown area in `data/coming.toml`, and (with `--panicOnWarning`, as in the workflow)
  on raw HTML in the markdown, which would be dropped: write `\<name>` or put it in backticks.

Preview locally with Hugo 0.170.0, from the repository root:

    hugo server --source site

and open http://localhost:1313/. `hugo --source site --panicOnWarning --destination <folder>` builds
the site into a folder, as the workflow does.
