# E69: a VST3 editor was refused before it could report its size (#295)

1. **Failure.** In v0.6.0 an editor that opened in v0.5.x no longer appeared; the log said
   `size=failed` and then `zero size`.
2. **Mechanism.** The #289 diagnostics change in `crates/vst3-host/src/ffi/view.rs` replaced
   the default pre-attach window size with an error. Some views only report a size once they
   have a parent. Following their size after attachment must also work when scaling is refused.
3. **Why checks missed it.** The VST3 fixture always reported a usable size before attachment.
   The CLAP zero-size refusal test covers a different GUI lifecycle contract.
4. **Regression test.** The two size-before-attach tests in
   `crates/vst3-host/src/ffi/editor_diagnostics_tests.rs` use fixture modes that return an error
   or an empty rect before attachment, then 640×400, without a resize callback. They check the
   default window, final content size, original probe fact, one fallback INFO and no WARNs.
   On the v0.6.0 base, opening fails with `Err("zero size")`.
5. **Gate class.** Diagnostics must not turn an optional pre-lifecycle query into a requirement.
   Exercise unavailable query results before and after the lifecycle transition.
6. **Protocol change, owner, blind spot.** Plug-in host maintainers keep these fixture modes in
   the Windows editor gate. Real plug-ins and other desktop window systems still need their
   platform checks; this fixture does not model a delayed asynchronous size change.
