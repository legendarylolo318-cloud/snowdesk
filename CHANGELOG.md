# Changelog

All notable changes to snowdesk are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project uses
[Semantic Versioning](https://semver.org/).

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

[0.1.0]: https://github.com/legendarylolo318-cloud/snowdesk/releases/tag/v0.1.0
