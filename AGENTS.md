# Design guidance

For changes to the application UI or plugins, follow the proposed design language in [docs/design-guide.md](docs/design-guide.md). Use the [design-system mockup](mockups/design-system.html) as a visual reference. The guide is a design direction; existing native components may not yet implement it.

# Releases

Every GitHub release body must list the release's changes, not only the auto-generated **Full Changelog** compare link. Use the matching `CHANGELOG.md` section under a `## Changes` heading, followed by the **Full Changelog** link. The release workflow publishes with `--generate-notes`, which produces only the link, so after it finishes set the body with `gh release edit <tag> --notes-file <file>` and check it with `gh release view <tag>`.
