# Upgrading Super Bevy World 64 from Bevy 0.12.1 to 0.19

Done. This is the record of what changed and, where it matters, why — kept
because seven releases' worth of renames is not something worth rediscovering
from compiler errors a second time.

## Result

| | |
| --- | --- |
| Was | `bevy = "0.12.1"`, Rust 1.75 |
| Now | `bevy = "0.19.0"`, Rust 1.95+ (built here on 1.96) |
| Tests | 88 passed, 0 failed — the same 88 that passed on 0.12 |
| Warnings | none, `cargo clippy --all-targets` included |
| Windows | `./build_windows.sh` produces a running `.exe` and the ZIP |

The route was 0.12 → 0.13 → 0.14 and then straight to 0.19. The first two hops
were taken one at a time and each ended green; the rest was done in one pass
against the 0.19 compiler.

## The toolchain

`/usr/bin/cargo` is the distro's 1.75 and cannot build this any more. The
rustup toolchain that was already installed alongside it can:

```bash
export PATH="$HOME/.cargo/bin:$PATH"   # rustc 1.96, and the Windows target
```

Nothing had to be downloaded. That also emptied out most of
`build_windows.sh`: it used to fetch an official 1.75 compiler and two standard
libraries into `target/` because the distro compiler and the official Windows
`rust-std` had mismatched internal identities. One rustup toolchain serving
both host and target is what that was imitating, so the script is now a
`rustup target add` and a `cargo build`.

## The three that were rewrites

**Animation.** `AnimationPlayer` stopped taking clip handles and started taking
node indices into an `AnimationGraph`. `CharacterAnimations` now maps a name to
an `AnimationNodeIndex` and holds one graph per character; `apply()` is still
the single choke point every caller goes through, but it takes an
`AnimationTransitions` component alongside the player, because the cross-fade
moved out of the player and into that. Repeat and speed became per-animation
rather than per-player and so are re-applied on every call — which matters
here, since the walk's rate tracks ground speed while the same clip keeps
playing. A new `attach_graphs` system hands each owned player its graph, kept
separate from `claim_players` because ownership is known when a scene spawns
and the graph does not exist until the glTF has loaded. Enemies share one graph
per *clip* via `EnemyGraphs`, so the five-thousand-enemy cap does not mean five
thousand graph assets.

**Text and UI.** Bundles are gone. `TextBundle` became `Text` + `TextFont` +
`TextColor` + `Node`, `Style` was folded into `Node`, `sections[0].value`
became writing through the `Text` component itself, and `ZIndex::Global(n)`
became `GlobalZIndex(n)`. In 0.19 specifically, parley replaced cosmic-text and
`font_size` became `FontSize::Px(..)`. The console came out shorter than it
went in.

**Input.** A gamepad is an entity carrying its own button and axis state, so
`Res<Gamepads>` + `Res<ButtonInput<GamepadButton>>` + `Res<Axis<GamepadAxis>>`
collapsed to `Query<&Gamepad>`, and `GamepadButtonType`/`GamepadAxisType`
became `GamepadButton`/`GamepadAxis`. Typed text moved off the deleted
`ReceivedCharacter` onto `KeyboardInput::logical_key`; `Key::Space` has to be
matched by name because it is not a `Key::Character`, and without that the
console silently refuses to type a space.

## Things the migration guides do not tell you

- **`tonemapping_luts` now requires naming a zstd backend** — `zstd_rust` or
  `zstd_c` — and naming neither is a hard `compile_error!`, not a fallback.
  `zstd_rust` keeps the MinGW cross-build free of a second toolchain.
- **`Mesh::new` takes `RenderAssetUsages`, and `MAIN_WORLD` is load-bearing for
  the water.** `drift` rewrites the sheet's UVs every frame through
  `Assets<Mesh>`; a render-world-only mesh has nothing on the CPU left to
  rewrite, so the lookup returns `None` and the water stops moving with no
  error anywhere.
- **`gpu-allocator` and `wgpu-hal` must agree on which `windows` crate they
  use.** Incrementally updating `Cargo.lock` across the hops attached
  `gpu-allocator` to the `windows 0.58` node that `gilrs` pulls in, while
  `wgpu-hal` used 0.62 — so `ID3D12Heap` from one was a different type from the
  other and the Windows build failed with ten trait errors inside a dependency.
  Deleting `Cargo.lock` and re-resolving fixed it. Worth remembering: the
  symptom points at wgpu, the cause is the lock.
- **The audio path is not covered by `cargo test` on Linux.** It is behind
  `sound`/`windows`, so `AudioBundle` and `Volume::new_relative` survived every
  green test run and only failed when the Windows build reached them. The
  Windows cross-build is the check for that module.
- `Window::cursor` became a separate `CursorOptions` component on the window
  entity, and `DirectionalLight::shadows_enabled` became `shadow_maps_enabled`.
- `Entity::from_raw` and `Query::single` are both fallible now, and
  `run_system_once` returns a `Result` — in the tests it is unwrapped, so a
  system that fails to run fails the test.

## Still to check on Windows

Everything below is invisible to a headless test, so it is what a first run
should be looking at:

- Billboards face the camera and are two-sided — the ordering that makes that
  work is now `.after(animate_targets).before(TransformSystems::Propagate)`.
- Skeletal animation plays and blends, and non-looping clips still do not loop.
- The console draws glyphs, edits, and pins controls; a space types.
- Water is transparent from underneath, drifts, and the underwater tint
  triggers off the camera.
