# Bareline local patch

Upstream: `accesskit_windows` 0.35.0, crates.io SHA-256
`a59e8d7160cbac991c1345c99153783ab96423644ccb1f20617cf80c97596aa1`.
Upstream git commit: `42e53b0d829e7a0b34dc8803bb06012db2e80cc6`,
`adapters/windows`. Original MIT, Apache-2.0 and Chromium license files retained.

The only provider extension is a weak per-HWND pattern factory consulted by
`PlatformNode::GetPatternProvider`, outside the consumer tree lock. A registration
guard removes its own entry, including when an HWND is reused. Bareline replaces
only editor Text/TextEdit patterns; AccessKit retains node identity, fragment
navigation, all chrome patterns and event publication. No registry cache is edited.

This is an application patch, not a claim that upstream provides this API.
