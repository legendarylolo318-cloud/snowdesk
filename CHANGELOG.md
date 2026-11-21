# Changelog

All notable changes to snowdesk are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

## [0.3.0] - 2026-11-20

### Added

- Snow rides along when you move a window gently, and flies off when you
  shake it (feeds now report window identity, and poll window geometry at
  20 Hz for a moment after a window moves).
- Icicles grow under snowy window edges over a few minutes, drip, and break
  off as ice shards when a window is shaken or closed (toggle in the Snow tab).
- Frost on the screen corners (optional): it creeps in while you are away
  and the pointer wipes it off or writes in it. Off unless a pointer position
  is available.
- Penguins skate and twirl on ice, walk from one monitor to the next (and
  thrown snowballs fly across), and start a snowball fight when you hit one:
  it and the penguins nearby throw snowballs back at your pointer.
- Brick tool: lay packed-snow bricks in a staggered pattern for forts. Igloo
  button: a hollow igloo with a tunnel, sized so penguins can walk in.
- In play mode the pointer leaves footprints where it skims the snow.
- React to any music on the PC, not only the built-in player: Linux via the
  speakers' monitor source (PulseAudio / pipewire-pulse), Windows via WASAPI
  loopback. Off by default; audio is only analysed in memory.

### Changed

- Penguins follow the ground at their feet: they no longer jump up onto
  window tops when walking underneath, they walk off ledges and fall, and
  walls (bricks, steep piles) turn them around.

## [0.2.1] - 2026-11-20

### Fixed

- Windows: the overlay failed to start ("tried to access window handle
  outside of the main thread"), so no snow appeared. The overlay thread now
  reads window handles in a thread-independent way, which also makes the
  click-through windows visible.
- Windows: snowdesk always showed "auto-paused (fullscreen app)". Its own
  monitor-sized overlay made Windows report a fullscreen app. Fullscreen
  detection now checks whether the foreground window covers its monitor, and
  pauses only that monitor.

## [0.2.0] - 2026-11-20

### Added

- Several penguins (1–12 per monitor) and a size setting (0.5×–3×), in the
  Snow tab and as `penguins` / `penguin_size` in config.toml.
- Penguin interaction in play mode: click to make one hop away, drag to carry
  it, flick to throw it, snowball it to knock it over. Penguins flap while in
  the air, lie down when knocked over, and walk along window tops.
- Stacking by hand: while rolling a snowball, move the pointer up to lift it,
  then release it over a bigger ball. Released balls snap onto the centre of
  the ball below.
- README screenshots and a play-mode GIF.

### Fixed

- Snow sliding off window ledges fell as grid cells that ignored the
  max pile height, so piles under window edges grew without limit in heavy
  snow. It now drifts down as flakes again. Piles on window tops are capped at
  about a third of the max height.
- Recognised snowmen no longer fall apart after a few seconds (balls stacked
  on spheres slowly rolled off).
- A snowball rolled underneath a window jumped onto the window's top edge;
  it now stays on the surface nearest the pointer.
- Penguin body parts were not scaled on HiDPI screens.

### Performance

- Wayland: a monitor blanked by a fullscreen app or game, or disabled in the
  panel, gives its swapchain memory back (VRAM 105 → 75 MB with one of two
  monitors covered, 28 MB with both), so games get the memory.
- Cursor wind polls the pointer at 60 Hz only while it moves, then 10 Hz:
  the Hyprland feed went from 176 to 30 wakeups/s and ~0 % CPU with the mouse
  still.
- Freed heap is returned to the OS after startup and music rescans, and glibc
  is limited to two malloc arenas: private RAM 20.5 → 14.6–17.8 MB.

## [0.1.0] - 2026-11-20

First public release.

### Features

- Transparent, click-through snow overlay on every monitor: Wayland
  layer-shell (Hyprland, Sway, KDE and other wlroots-style compositors), X11,
  and Windows 10/11.
- Falling flakes with depth, sway, wind, random gusts and cursor wind; settled
  snow piles up on the bottom of each screen and on the top edges of real
  windows. It falls off when a window moves, minimizes or closes.
- Play mode with an in-overlay tool bar: brush, shovel, pack, freeze, melt,
  rolling snowballs that grow and stack, snowmen with decorations (eyes, nose,
  arms, hat, buttons, scarves), a quick-stamp snowman, a mountain generator,
  right-click flick to throw, snow angels, 30-step undo/redo, clear, and
  save/load of the scene.
- Festive extras: a penguin that leaves footprints and sleds, string lights on
  window edges, ice sparkles and a night-time aurora.
- Music player for your own files (mp3, ogg, flac, wav, m4a/aac) with tags,
  cover art, shuffle, repeat, volume and seek; the folder is watched live.
  Bass hits drive gusts and bursts of snow.
- Fullscreen apps and games pause the snow on their monitor; when every
  monitor is covered, nothing is simulated or rendered.
- Tray icon, single-instance IPC (`--toggle-play`, `--pause`, `--clear`,
  `--next-track`, `--show`, `--quit`), TOML config with hot reload, and a
  start-with-desktop toggle.

### Fixed during the release audit

- X11 and Windows: the overlay and the control panel each created a winit
  event loop, and winit allows only one per process, so the panel failed with
  "EventLoop can't be recreated". The overlay windows now live in the panel's
  event loop (or a small standalone loop with `--no-panel`) and are rendered
  from the overlay thread. GNOME Wayland (no layer-shell) now switches the
  whole app to XWayland instead of failing.
- Windows: the overlay could never be transparent: wgpu's default DX12
  swapchain has no alpha. snowdesk now uses a DirectComposition swapchain, sets
  layered-window attributes after toggling click-through, builds with the
  static C runtime (no vcruntime140.dll), and opens no console window.
- Throwing a snowball that touched the ground and another ball in the same
  step removed it twice (wrong ball removed, or a crash).
- A single snowflake landing at the edge of a snowman's base could roll the
  whole snowman over. Balls now roll on the real terrain slope.
- Damaged `scene.bin` files could make the app allocate gigabytes, divide by
  zero, or load NaN balls. Scene loading now validates everything.
- Removing one of two overlapping windows cleared the other window's ledge.
- Snow piled up on edges hidden behind other windows (it showed on top of the
  front window). Ledges now follow the stacking order, and the control panel
  hides the edges behind it without collecting snow itself.
- Hyprland: windows on a visible special workspace (scratchpad) were ignored.
  X11: windows on other desktops still had ledges.
- Seeking once broke the seek bar's link to the playback position.
- After files were added to the music folder, the wrong track was highlighted
  and seek used the wrong track length.
- Cover art: a stale image was shown (cached file path), the temp files went
  to a shared `/tmp`, and full-size covers stayed in memory. Art is now decoded
  in memory and scaled to 176 px.
- Audio on Linux systems without pipewire-alsa: the default ALSA device
  failed and the fallback picked ALSA's "null" sink, so every track
  "finished" in a second with no sound. Linux builds now talk to
  PulseAudio / PipeWire (pipewire-pulse) natively, keep ALSA as the fallback,
  and never use the null sink.
- Audio: one corrupt file stopped "next"; a failed output was retried per
  track; a lost audio device was never reopened. Playback now skips unreadable
  files, resumes on a new device, and an idle player no longer wakes up.
- A client that connected to the IPC socket and stayed silent blocked every
  later command. The socket is now removed on exit.
- `--no-panel` never drained audio events, so memory grew while music played.
- A GPU device loss or a panic used to stop the overlay for good. It is now
  rebuilt (up to three times in two minutes). If the overlay ever stops
  responding while in play mode, a watchdog ends the process so the screen
  cannot stay blocked. Quitting never hangs.
- The desktop-feed thread was never stopped. The config is saved atomically.
  The autostart `Exec=` line is escaped properly.
- Other UI and tool fixes: the radius indicator now reflects the radius, the
  tool bar shrinks to fit narrow screens, and the ball count is capped so huge
  builds stay fast.

### Performance

Measured on Hyprland with two monitors (2560×1440 and 1920×1080), Radeon RX
9070 XT, 60 fps cap. CPU is the percentage of one core.

| Scenario | Before | After |
| --- | --- | --- |
| Idle, passive (panel hidden) | 1.7 % CPU, 112 MB RSS, 297 MB VRAM | 1.7 % CPU, 90 MB RSS, 105 MB VRAM |
| Blizzard (intensity 100 %) | 2.7 % CPU, 113 MB RSS, 297 MB VRAM | 2.7 % CPU, 90 MB RSS, 105 MB VRAM |
| Paused | 0 % CPU, 108 MB RSS, 276 MB VRAM | 0 % CPU, 86 MB RSS, 83 MB VRAM |
| Panel open | 1.9 % CPU, 143 MB RSS, 374 MB VRAM + 26 MB GTT | 1.7 % CPU, 105 MB RSS, 105 MB VRAM + 2 MB GTT |
| Play mode, 200 balls, 40 snowmen, mountains | not measured | 4.3 % CPU, 91 MB RSS, 105 MB VRAM |

- The GPU allocator uses small memory blocks: wgpu's default reserves
  128–256 MB blocks.
- Only the native GPU API (Vulkan / DX12) is initialized. Starting wgpu's GL
  backend loaded Mesa's GL driver and LLVM (about 35 MB) for nothing.
- The control panel uses Slint's software renderer, which saves about 40 MB RAM
  and 76 MB VRAM over the GPU renderer with no visible difference.
- Settled snow is shaded only below the highest pile instead of across the
  whole screen; adjacent dirty chunks are uploaded in one copy.
- The panel wakes only when there is something to show (no 16 Hz timer), and
  the music player stops ticking when nothing is loaded.
- Undo snapshots are run-length encoded: under 1 MB for 30 steps instead of
  about 7 MB per 1440p monitor.
- The music library keeps a tag cache, so rescans of large folders only read
  new or changed files. The track list is virtualized.

[0.3.0]: https://github.com/legendarylolo318-cloud/snowdesk/releases/tag/v0.3.0
[0.2.1]: https://github.com/legendarylolo318-cloud/snowdesk/releases/tag/v0.2.1
[0.2.0]: https://github.com/legendarylolo318-cloud/snowdesk/releases/tag/v0.2.0
[0.1.0]: https://github.com/legendarylolo318-cloud/snowdesk/releases/tag/v0.1.0
