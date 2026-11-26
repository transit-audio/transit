<!-- transit-index
status: Open, cosmetic (#727)
follow-up: Accepted as a cosmetic limit (packaging/macos/dmg/README.md). Eject the older transit disk image before opening the new one.
-->

macOS: two transit disk images mounted at once can show each other's background (#727). The window layout finds the background by the volume name, which every transit disk image shares, so the new image's window can show the older image's background. Cosmetic: the app inside is the new build.
