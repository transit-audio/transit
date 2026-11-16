<!-- transit-index
status: Open, needs a Mac (#674, #727)
follow-up: A tester opens the v0.7.3 disk image on a Mac and reports whether the app opens without the quarantine workaround and whether the window background shows the transit wordmark.
-->

macOS: the v0.7.3 build is the first signed with a Developer ID and notarized by Apple (#674). A release-workflow run on 2026-11-16 got the app and the disk image accepted for notarization, both were stapled, and the entitlement check passed on the app and on the copy inside the disk image; opening it on a Mac has not yet been confirmed. The disk image window background showed the inherited project's wordmark and artwork until #727; the source and image now carry transit's, not yet seen on a Mac. If macOS still blocks the app, run `xattr -dr com.apple.quarantine /Applications/transit.app` or right-click it and choose Open.
