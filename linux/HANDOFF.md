# Session Handoff — 2026-03-17

## What We Were Doing
Replacing VTE (CPU-based Cairo renderer) with Ghostty's GPU-accelerated OpenGL renderer in cmux-linux. The macOS version already uses Ghostty via Metal; this brings the same renderer to Linux via the libghostty C embedding API.

## Current State
- **Branch**: `main` (uncommitted changes)
- **Working**:
  - libghostty.so builds for Linux (`cd ghostty && zig build -Dapp-runtime=none -Doptimize=ReleaseFast`)
  - Rust FFI crate links and compiles
  - App launches without crashing
  - Terminal renders with Ghostty OpenGL (prompt visible, can type after clicking)
  - Tab title updates from shell
  - WebKit browser tabs (with sandbox workaround)
- **Broken/Incomplete**:
  - **Terminal doesn't resize** when window is resized — stays at initial size, leaves black gap
  - **Focus lost after resize** — can't type until clicking terminal again
  - Ctrl+C may cause segfault (needs investigation)
  - Clipboard (copy/paste) untested
  - Mouse selection untested

## Key Decisions Made
- **Used Ghostty's embedded C API** (not the GTK apprt's GhosttySurface widget) because we have our own GTK app shell with workspaces/tabs/splits
- **Added `GHOSTTY_PLATFORM_LINUX` to the embedded API** — the Ghostty fork needed changes to support Linux in the embedded apprt (previously macOS/iOS only)
- **Fixed OpenGL `surfaceInit`** to call `prepareContext(null)` for the embedded apprt — was a no-op TODO that caused "broken rendering"
- **Set `must_draw_from_app_thread = true`** on Linux in the embedded App — OpenGL via GTK GLArea requires main-thread draws
- **Compiled glad (GL loader) separately** via cc crate — libghostty.so doesn't include it when built as a shared library
- **Set `GDK_DISABLE=gles-api,vulkan`** before GTK init — Ghostty requires desktop GL 4.3+, GTK defaults to GLES which crashes glad
- **Set `WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1`** — WebKitGTK's bubblewrap sandbox fails without unprivileged user namespaces

## Files Changed This Session

**Ghostty fork (submodule):**
- `ghostty/src/apprt/embedded.zig` — Added Linux platform, `must_draw_from_app_thread`, Platform.C union
- `ghostty/src/renderer/OpenGL.zig` — Changed embedded apprt surfaceInit from no-op to `prepareContext(null)`

**Repo root:**
- `ghostty.h` — Added `GHOSTTY_PLATFORM_LINUX`, `ghostty_platform_linux_s`, union field

**New crate:**
- `linux/rust/cmux-ghostty-sys/Cargo.toml` — FFI crate config, depends on `cc` for glad
- `linux/rust/cmux-ghostty-sys/build.rs` — Links libghostty.so, compiles vendor/glad/src/gl.c
- `linux/rust/cmux-ghostty-sys/src/lib.rs` — Manual FFI bindings to ghostty.h (structs, enums, extern fns)

**Modified:**
- `linux/Cargo.toml` — Added cmux-ghostty-sys to workspace
- `linux/rust/cmux-host-linux/Cargo.toml` — Replaced vte4 with cmux-ghostty-sys
- `linux/rust/cmux-host-linux/src/main.rs` — Added `init_ghostty()`, GDK env vars, webkit sandbox workaround
- `linux/rust/cmux-host-linux/src/terminal.rs` — Complete rewrite: VTE → Ghostty GLArea with input/render/resize
- `linux/rust/cmux-host-linux/src/pane.rs` — Removed vte4 imports, wired TerminalCallbacks for title/bell/close
- `linux/rust/cmux-host-linux/src/window.rs` — Removed VTE Shift+Enter workaround, removed `use vte4::TerminalExt`

## Blockers & Workarounds

**FIXED — Resize was broken by sync draw guard:**
`ghostty_surface_draw()` calls `CoreSurface.draw()` which calls `drawFrame(true)` (sync=true). Inside `drawFrame`, there's a macOS-specific guard: `if (sync && size_changed) → presentLastTarget() → return`. This re-presents the old frame to avoid CoreAnimation blank flashes during resize. But it means the renderer NEVER renders at the new size when called via the embedded API. Fix: changed `embedded.Surface.draw()` to call `self.core_surface.renderer.drawFrame(false)` instead of `self.core_surface.draw()`. The `false` (non-sync) path lets the renderer detect the new GL viewport and resize its FBO. File: `ghostty/src/apprt/embedded.zig` line ~809.

**Struct size mismatch was the prior crash cause:**
`ghostty_surface_config_s` had 3 missing fields (`io_mode`, `io_write_cb`, `io_write_userdata`), causing stack corruption. Fixed by adding them. The built header at `ghostty/zig-out/include/ghostty.h` is authoritative — the repo-root `ghostty.h` is manually maintained and outdated.

**GLES vs GL:**
GTK4 defaults to GLES. Ghostty's glad loader crashes with GLES. Must set `GDK_DISABLE=gles-api` BEFORE GTK initialization. `set_use_es(false)` is deprecated and ignored on GTK 4.14+.

## Next Steps

1. **Fix resize** — The terminal viewport must update when the window resizes. Debug by adding prints in the resize callback to verify dimensions. Compare the values being passed to `ghostty_surface_set_size` with what Ghostty's GTK apprt passes. May need to `queue_render()` after size change, or the issue may be in the OpenGL renderer not updating `glViewport`.
2. **Fix focus loss on resize** — GTK relayout during resize may steal focus from GLArea. May need to re-grab focus in the resize handler or use a focus controller.
3. **Test and fix Ctrl+C** — Verify whether it's a segfault in Ghostty or expected app quit behavior (last tab closing → window closes).
4. **Test clipboard** — copy/paste via Ctrl+Shift+C/V, selection clipboard.
5. **Commit the Ghostty submodule changes** — Push to manaflow-ai/ghostty fork before committing the parent repo pointer.
6. **Commit all changes** to main.

## Key Context the Next Session Needs

**Build & run command:**
```bash
cd ~/Applications/cmux-linux/cmux/linux
# Only needed if ghostty changes: cd ../ghostty && zig build -Dapp-runtime=none -Doptimize=ReleaseFast && cd ../linux
cargo build --release --features webkit
LD_LIBRARY_PATH=../ghostty/zig-out/lib:$LD_LIBRARY_PATH ./target/release/cmux-linux
```

**Zig 0.15.2** is at `/snap/bin/zig`.

**The authoritative C header** is `ghostty/zig-out/include/ghostty.h` (generated by zig build), NOT `ghostty.h` at repo root. Use the built one to verify struct sizes.

**Check struct sizes** with:
```bash
cd ~/Applications/cmux-linux/cmux && gcc -U linux -I ghostty/zig-out/include /tmp/offsets.c -o /tmp/offsets && /tmp/offsets
```
(`-U linux` needed because GCC predefines `linux` as a macro, which conflicts with the `linux` union field in `ghostty_platform_u`)

**Plan file** at `/home/willr/.claude/plans/lucky-drifting-piglet.md` has the full architecture plan.
