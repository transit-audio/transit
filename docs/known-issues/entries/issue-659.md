<!-- transit-index
status: Open, needs a Mac
follow-up: A tester on a Mac loads a VST3 or CLAP plug-in signed by its own vendor in the v0.7.2 app and reports; the issue closes on that.
-->

macOS: loading third-party VST3 and CLAP plug-ins (#659). The v0.7.1 app was signed with the hardened runtime and without the entitlement that lets it load plug-ins signed by other vendors, so macOS refused every one and the device menus found none. v0.7.2 signs the app with that entitlement, and the release build checks the signed app and the copy inside the disk image for it, but the fix has not been confirmed on a Mac. The app is ad-hoc signed and not notarized.
