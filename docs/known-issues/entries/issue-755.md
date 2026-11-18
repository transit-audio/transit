<!-- transit-index
status: Open, macOS (#755)
follow-up: Use the VST3 or CLAP build of the plug-in. Audio Unit hosting is planned work in #755 and needs a Mac to build and test on.
-->

macOS: Audio Units are not usable yet (#755). A unit loads and processes audio, but its editor window cannot be opened, sidechain inputs and extra buses are not connected, units that require asynchronous instantiation are refused, and note handling has two open defects (#561, #602). VST3 and CLAP hosting work on macOS.
