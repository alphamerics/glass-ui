# glass-ui

Reusable liquid glass UI kit for Windows Tauri apps. Two halves in one repo:

- **`src/` + `styles.css` + `fonts/`** — the npm package: React components
  that render **native GPU liquid glass surfaces** behind the DOM (via the
  [`liquid-glass`](https://github.com/alphamerics/liquid-glass) crate), the
  Tauri invoke bridge they speak, and the shared glass design system (SF Pro
  Display, menu, modal and glass-shape styles).
- **`native/`** — the Rust crate `glass-ui`: `MenuGlassManager`, the
  menu-glass machinery (per-key glass surfaces, warm recycle pool, grouped
  reveals, scale spring, hit rects, reconciliation sweep) plus optional
  Tauri command wrappers behind the `tauri` feature.

## Consuming (from any app)

### TypeScript

```jsonc
// package.json — while iterating locally (sibling checkout):
"dependencies": { "glass-ui": "file:../glass-ui" }
// standalone (published state):
"dependencies": { "glass-ui": "github:alphamerics/glass-ui#v0.1.0" }
```

Then in the app root:

```tsx
import "glass-ui/styles.css";
import { GlassCard, LiquidDropdown } from "glass-ui";
```

`styles.css` owns the SF Pro Display `@font-face` rules (font URLs resolve
inside the package) plus the base transparent-document styles, the menu,
modal and glass-shape classes. Apps bring their own layout CSS.

### Rust

```toml
# src-tauri/Cargo.toml — while iterating locally (sibling checkout):
glass-ui = { path = "../glass-ui/native", features = ["tauri"] }
# standalone (git dep, tracks main):
glass-ui = { git = "https://github.com/alphamerics/glass-ui", features = ["tauri"] }
```

And wire it up:

```rust
use glass_ui::{commands, MenuGlassManager};

tauri::Builder::default()
    .manage(MenuGlassManager::new())
    .invoke_handler(tauri::generate_handler![
        commands::set_ui_hit_rects,
        commands::prewarm_menu_glass,
        commands::position_menu_glass,
        commands::sync_menu_glasses,
        commands::log_message,
    ])
    // optional: grouped surfaces emit when the whole group is warm, so the
    // web UI can fade its content in with the glass.
    .setup(|app| {
        app.state::<MenuGlassManager>()
            .set_group_emitter(|| { /* e.g. app.emit("glass-group-ready", ()) */ });
        Ok(())
    })
```

## Glass app setup checklist

Every app that renders liquid glass needs all of the following, or the
glass will be wrong (refracting its own DOM, mis-scaled, or absent):

1. **Transparent, decorationless overlay window** — in `tauri.conf.json`:
   `transparent: true`, `decorations: false`, `backgroundColor: "#00000000"`,
   `shadow: false`.
2. **`--force-device-scale-factor=1`** in the window's
   `additionalBrowserArgs` (the kit pushes rects in physical px; a DPI-scaled
   DOM would misplace every surface). Recommended alongside:
   `--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection,CalculateNativeWinOcclusion`.
3. **Exclude the web UI from the glass's own capture** — at startup:
   `SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE)`, and set
   `WS_EX_LAYERED` on the window (what lets cross-thread click-through work
   for shell overlays).
4. **Ship `glass-broker.exe` beside the app executable** — the broker bin
   must exist next to the app at runtime. Apps that embed their own
   broker trampoline as a `[[bin]]` target get it for free: Tauri
   automatically bundles every bin target beside the executable (build it
   first in `beforeDevCommand`/`beforeBuildCommand`, like Glassbar does).
   Apps consuming the library differently bundle it explicitly:

   ```jsonc
   "bundle": { "resources": { "target/release/glass-broker.exe": "glass-broker.exe" } }
   ```

   The glass pipeline auto-spawns the broker (session-wide singleton; many
   glass apps share one).
5. **Full-screen click-through shells only:** push interactive-element rects
   via `set_ui_hit_rects` and run a hit-test loop that makes the overlay
   click-through everywhere else. Normal (non-shell) apps skip this.

## Multi-repo state (shipped)

The crates are git-based now. `native/Cargo.toml` depends on
`liquid-glass = { git = "https://github.com/alphamerics/liquid-glass" }` —
there is deliberately NO `[patch]` committed here: a git-fetched package
cannot resolve path dependencies outside its own checkout, so a patch in
this manifest would break every crate consuming `glass-ui` from GitHub.

Live local iteration lives in the **consuming app** instead (glassbar is an
app, never a dependency, so committing patches there is safe):

```toml
# glassbar/src-tauri/Cargo.toml
[dependencies]
liquid-glass = { git = "https://github.com/alphamerics/liquid-glass" }
glass-ui = { git = "https://github.com/alphamerics/glass-ui", features = ["tauri"] }

# redirect both to the sibling checkouts so edits apply instantly:
[patch."https://github.com/alphamerics/liquid-glass"]
liquid-glass = { path = "../../liquid-glass" }
[patch."https://github.com/alphamerics/glass-ui"]
glass-ui = { path = "../../glass-ui/native" }
```

Working with a Rust `liquid-glass` copy that is NOT tracked main (e.g. local
edits ahead of a push): glassbar's committed patches already route around
this — the patched path is used instead of the fetched repo.

If the GitHub repos are private, cargo's built-in fetch cannot use your
credential helper by itself; enable the CLI fallback once per machine:

```toml
# ~/.cargo/config.toml
[net]
git-fetch-with-cli = true
```

The npm package stays on the sibling `file:` dep (glassbar's choice): instant
edits, no link step. A standalone consumer switches package.json to
`github:alphamerics/glass-ui#v0.1.0` as shown above.

The npm package needs no build step in git-dep mode: `package.json` has a
`prepare` script pattern (`tsc` emit is unnecessary — apps consume the TS
source through their own bundler; `main`/`types` point at `src/index.ts`).
