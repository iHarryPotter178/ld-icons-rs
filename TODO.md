# TODO

- [x] **IPC integration with nsd daemon**: `NsdClient` monitors mount/unmount events and manages drive icons on desktop with configurable file-manager opener.

- [ ] Validate multi-monitor support in a real setup (currently untested).
	- Test scenario: second physical monitor connected (different make/model, mixed resolution).
	- Test scenario: monitor hotplug while `ldicons` is running.
	- Test scenario: different Wayland display/socket (e.g. non-default `WAYLAND_DISPLAY`).
	- Test scenario: drag icons across monitor boundaries and verify redraw/input regions.
	- Test scenario: per-monitor scale differences (HiDPI + non-HiDPI combination).

- [ ] Add optional swap behavior for grid snapping: if a target cell is already occupied during drag-and-drop, swap the two icons instead of moving to the next free cell.
- [ ] Add real locale-based hyphenation instead of purely character-based splitting for long words.
- [ ] Introduce proper UI localization with gettext (`.po`/`.mo`) instead of hardcoded strings: includes context-menu labels, tooltip fallbacks (e.g. trash), and other visible UI text.
- [ ] Extend status-emblem system to support additional configurable emblem types (without code changes), including per-type priority and icon-name mapping.
- [ ] Change CLI defaults for `--conf` and `--pos` to `~/.config/deskicons/` instead of workspace paths.
