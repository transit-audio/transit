<!-- transit-index
status: Closed (#659, v0.7.2)
follow-up: Done: the reporter confirmed on a Mac that the v0.7.2 build loads VST3 and CLAP plug-ins; signing with a Developer ID and notarization are tracked in #674.
-->

macOS: loading third-party VST3 and CLAP plug-ins (#659). The v0.7.1 app was signed with the hardened runtime and without the entitlement that lets it load plug-ins signed by other vendors, so macOS refused every one and the device menus found none. v0.7.2 signs the app with that entitlement, and the release build checks the signed app and the copy inside the disk image for it; the reporter confirmed on a Mac that the v0.7.2 build loads such plug-ins.
