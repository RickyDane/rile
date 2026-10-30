# Rile website

A standalone, dependency-free static site. Serve this folder with any static host:

```sh
python3 -m http.server 8877 --directory website
```

Open `http://localhost:8877` in Winfer's browser. No build step or external
runtime services are needed; fonts, artwork, icons and screenshots are local.
Download buttons point to the verified v0.8.1 release assets. Update these links,
and the version text in `index.html` together for future releases.

## GitHub Pages deployment

The site's Pages URL is <https://rickydane.github.io/rile/>, deployed by
`.github/workflows/pages.yml`. GitHub Pages is configured to use **GitHub Actions**.

- Push changes under `website/` (or the workflow itself) to `master` to deploy.
  The first deployment requires committing and pushing the website and workflow.
- Or run **Deploy website to GitHub Pages** from the Actions tab, selecting `master`:
  `gh workflow run pages.yml --ref master`.
- The workflow runs the DOM smoke tests before uploading only `index.html`,
  `assets/`, `css/` and `js/`. Development tools and this README are not published.
- Deployment uses the `github-pages` environment and the built-in `GITHUB_TOKEN`;
  no personal access token or separate publishing branch is required.

All asset links are relative, so the `/rile/` project path needs no build-time rewriting.

## Checks

```sh
node --check website/js/main.js
npm install --prefix /tmp/rile-website-check --no-audit --no-fund linkedom
node website/tools/check.mjs
```

The lightweight DOM smoke test checks asset paths, anchors, initialization,
mobile navigation, keyboard tour/radio selection, all 24 icon/accent combinations,
screenshot zoom/focus restoration, clipboard success/denial and desktop/mobile
platform detection, with full and reduced motion. It does not replace browser review.

For live regression checks, install `axe-core` alongside the test dependency:

```sh
npm install --prefix /tmp/rile-website-check --no-audit --no-fund axe-core
```

Then execute `tools/browser-check.js` with Winfer's `browser_run_code_unsafe`
`filename` option while this site is selected. It uses only the existing Winfer
browser, checks seven viewport widths (320–1920 px), moving decoration overflow,
download card styling, image loading, reduced motion, WCAG A/AA axe rules, and
the JavaScript-disabled fallback. It restores the viewport and motion preference.

Verify keyboard navigation (including arrows within tabs and radio groups),
Escape/focus restoration in screenshot zoom, narrow layouts, and reduced motion.
Animation is decorative; all content and download links work without JavaScript.

## Images and attribution

All nine images in `assets/screens/` were freshly captured on 2026-10-30 from
the current native macOS app, built with `cargo build -p rile-app` (v0.8.2-beta.1).
The session used an isolated `RILE_CONFIG_DIR` and generated demonstration files
under `/tmp/Rile Space/Mission Control`. Home shows the current statistics,
device cards and activity panels, not the old v0.6.0 interface. The download
links remain on the stable v0.8.1 release; screenshots show the development build.

Windows were captured at 1200×750 or 1440×900 logical pixels with
`screencapture -x -o -l <window-id> <file.png>` and resized to 1600×1000 WebP.
No application interface or values were composited. Captions and alt text describe
the actual contents. Device/storage values belong to the capture machine.

The space artwork was procedurally generated with `tools/artwork.html`.
The app icons are from `crates/rile-app/assets/app-icons/`.
Space Grotesk and JetBrains Mono are bundled under the SIL Open Font License;
see `assets/fonts/` for their licenses.
