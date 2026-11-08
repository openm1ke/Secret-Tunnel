---
name: android-product-design
description: Design and implement native Android product interfaces, including navigation, editable profiles, live network status and accessible application selectors. Use for substantial Android UX redesigns with working Kotlin implementation and emulator validation.
---

Translate the user's references into a coherent native Android interface and implement it in the existing project. Honor its network engine, supported Android versions, and existing credentials. This skill does not grant external access or require delegation.

For a tunnel client, distinguish saved configuration from the immutable running session. Server identity, host pin and encrypted client credential belong to the individual server profile. A local proxy profile names a server and listening port; selecting or editing a different profile must not silently change an active connection. Preserve old settings through a tested migration.

Use distinct, stable destinations for connection overview, server profiles, proxy profiles, applications and diagnostics. Keep SSH implementation detail in settings or diagnostics. Show an honest connected/connecting/reconnecting/stopped/error state, one primary connection action and actionable empty/error states. Never label a merely configured tunnel as connected.

Use the references' visual direction with restrained color, consistent spacing, readable text, contrast, 48dp touch targets and content descriptions for icon actions. Prefer native controls and platform fonts. Recycle application rows, load icons only for visible rows, preserve search and selection, detach switch listeners before rebinding recycled views.

Derive speed from monotonic-time counter deltas; handle resets and background gaps. Label rates and cumulative totals distinctly, draw upload/download with separate colors and keep only bounded chart history. Do not repeatedly assign unchanged text or run networking on the main thread.

Validate the real behavior: configuration migration, credentials isolated per server, profile validation and references, switch recycling, connect/stop/reconnect states, SSH host verification, and selected-app TCP/UDP. Inspect emulator screenshots at phone size; compile and lint the final installable APK. Identify emulator-only performance limits without implying physical-device testing.
