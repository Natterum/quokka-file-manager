# Changelog

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.4.1] — 2026-11-26

### Added

- Colour themes: "Settings → Appearance → Theme" is now one drop-down list with light and dark variants of 16 presets — Classic, Graphite, Ocean, Forest, Sunset, Lavender, Rose, and IDE- and web-style looks: Studio, Charcoal, Midnight, Dusk, Glow, Nordic, Solar, Ember and Contrast. A preset sets the accent colour (buttons, links, selection, focus), the background tint and, for the dark variants, how deep the background is; each entry shows a preview swatch.
- Settings transfer: "Import…" and "Export…" at the bottom of the settings window save all settings to a file and load them on another computer — favourites, columns, hotkeys, appearance, saved server connections and the rest. Open tabs of this computer are kept; saved passwords are not transferred.

### Fixed

- Saved server connections could disappear after an update. If the settings file was briefly locked (for example by an antivirus right after the new version started), Quokka fell back to the defaults and overwrote the file with them. Now the read is retried, a file that cannot be read is never overwritten, settings are written to a temporary file first and the previous version is kept as `settings.bak`, which is used if the main file is missing or empty. Connections saved by another Quokka window are no longer lost when this one saves.
- Find files (Alt+F7) always starts in the folder of the active pane; previously it could keep the folder of the last search while its results were shown.

## [0.4.0] — 2026-11-24

### Added

- Dark theme: "Settings → Appearance → Theme" switches between light and dark. It is applied immediately to all Quokka windows, including dialogs, menus, tooltips and notifications.
- Interface font: "Settings → Appearance → Font" offers the built-in Montserrat and Windows fonts (Segoe UI, Arial, Calibri, Verdana, Tahoma, Trebuchet MS, Georgia, Candara); only fonts installed on the system are listed.
- Corner rounding: "Settings → Appearance → Corner rounding" scales the rounding of buttons, fields, windows and lists from 100 % (as designed) down to 0 % (square corners), with a reset button.
- "Disconnect from …" in the context menu of a server or device tab.

### Changed

- Disconnecting from a server closes its tabs instead of leaving them pointing at a closed connection; a pane with nothing else keeps one tab and returns it to the last local folder from its history (or "This PC"). The server is also removed from the back/forward history.
- Closing the last tab of an SFTP/FTP server closes its connection — right away, or as soon as the operation queue no longer uses that server.
- The "About" window shows the changes of the installed version.
- The changelog is now in English.
- Built with Rust 1.100; all crates moved to edition 2024.
- Updated dependencies: eframe/egui 0.29 → 0.37 (the renderer is still OpenGL/glow), ureq 2 → 3 (update checks still use the Windows certificate store), notify 6 → 9, resvg 0.44 → 0.50, russh-sftp 3 → 4, windows-sys 0.59 → 0.61, plus compatible updates of the remaining crates.

## [0.3.1] — 2026-11-22

### Added

- Quick view (Ctrl+Q, "View → Quick view"): the other panel shows the file under the cursor and follows it as it moves. Photos are decoded in the background at screen size, recent ones are kept in memory, and neighbouring images are prepared in advance, so you can browse a folder with the arrow keys without waiting. Text files show their beginning; other files show an icon, type, size and date. Also works in single-panel mode.

### Fixed

- Arrow-key navigation no longer lags: the cursor is redrawn in the same frame rather than on the next input event, and no key presses are lost during fast auto-repeat.
- Dragging to other applications: marked files can be dragged by any part of the row, as in Explorer, not just by the icon or name. Handing the drag over to Windows also works at the very edge of the window, including when it is maximized.

## [0.3.0] — 2026-11-22

### Added

- "Show Windows menu" in the context menu of files, folders and empty space (Shift+F10): the full Explorer menu with all commands from Windows and installed programs — "Open with", "Send to", archivers, antivirus scans and so on. Holding Shift shows the extended commands.
- Drives in the sidebar: right-click offers "Open", "Open in new tab", "Add to favorites", "Eject" (for USB drives and optical drives), "Show Windows menu" and "Properties" (the Windows properties dialog). Local favorites also have "Properties" and the Windows menu.
- Drive tooltip on hover: a compact card with the volume label, used (red) and free (blue) space above a shared bar, and total capacity below it. While the cursor stays on the drive, the numbers refresh once a second. A thin fill bar is also shown on the drive button itself.
- Connecting and disconnecting devices (USB drives, memory cards, mounted images, network drives) is detected automatically: the drive list in the sidebar and in "This PC" updates, and an in-window notification offers to open the new device right away. Tabs on a disconnected drive switch to "This PC".
- Phones, cameras and players connected via MTP (without a drive letter) appear in the sidebar and in "This PC" as `mtp://Device name/…`: browsing folders, copying from and to the device, deleting, creating folders and renaming through the operation queue, and the storage's free space in the tooltip.
- Remappable keyboard shortcuts: "Settings → Keyboard shortcuts". Click a shortcut to record a new one, "+" adds another, right-click removes it. If a shortcut is already taken, Quokka names the command it belongs to and, after confirmation, moves it over, removing it from the previous command. Reset a single command or all of them at once. Menus, the F-key bar at the bottom and help (F1) show the current shortcuts. Arrows, Enter, Esc, Tab, Space and Ctrl+C / X / V cannot be remapped.

### Fixed

- Folder size calculation is no longer interrupted: a panel refresh caused by changes on disk (frequent in the home folder and on the system drive, which have many inaccessible folders) used to cancel the calculation and wipe the sizes already computed. Folders without read permission are skipped, and the size is shown as "≥ …" with a tooltip giving the number of skipped items.

## [0.2.0] — 2026-11-21

### Added

- SFTP, FTP and FTPS (explicit TLS) servers: "Tools → Connect to server…" (Ctrl+K) opens a server folder in a panel tab like any other. Saved connections, login by password or key file (with no password, keys from `~/.ssh` are used), confirmation of the server key fingerprint with an entry in `known_hosts`. Passwords can be remembered — they are stored in Windows Credential Manager, not in the settings file. Copying, moving and deleting go through the operation queue: download, upload and server-to-server transfers. For SFTP there is "SSH terminal" (Ctrl+Shift+P) in PuTTY or the built-in Windows `ssh.exe`.
- "Properties" in the context menu (Ctrl+Alt+Enter, "File → Properties"): the standard Windows properties dialog for a file, folder or drive; for several marked items, a single combined dialog, as in Explorer. "Folder properties" in the empty-space menu opens the properties of the current folder.
- Checking for and installing updates: once a day at startup and via "Help → Check for updates…", Quokka asks GitHub Releases for a new version and shows the list of changes. "Update and restart" downloads the package, verifies its SHA-256, installs it and restarts the program with the same tabs: an MSI installation is updated with the new MSI, a portable one by replacing quokka.exe. A version can be skipped; checking can be turned off in "Settings → View and files → Updates".
- MSI installer for x64 and x86 (WiX): Program Files, a Start menu shortcut, in-place upgrade of the previous version into the same folder. GitHub releases include the MSI next to the ZIP, and CI prepares winget manifests and, if the `WINGET_TOKEN` secret is present, submits them to winget-pkgs.
- Batch image conversion (Ctrl+Shift+E, context menu, "File"): JPEG, PNG, WebP (lossy and lossless), AVIF, BMP, TIFF, GIF, ICO, TGA, QOI. Quality 1–100 for JPEG/WebP/AVIF, compression level for PNG. By default the result is saved next to the source file with `_conv` appended to the name (configurable in the dialog): `photo.png` → `photo_conv.webp`; running again on the same folder does not reconvert earlier results. Output can go to a separate folder, in which case the subfolder structure is preserved. Folders are processed recursively and files are converted in parallel through the operation queue. EXIF orientation is applied, transparency in JPEG is filled with white, and the modification date is preserved. The dialog estimates the resulting size from a sample of files and recalculates it when the format or quality changes. A corrupted file does not stop the batch.
- A separate modal window for column settings: reorder by dragging with an insertion marker, visibility checkboxes, "Apply", "Cancel" and reset to the default set. Added creation date, access date, extension and file attributes, with sorting by the new columns. The set, order and widths are saved separately for each panel; old settings are migrated automatically.
- ZIP as a folder: Enter opens the archive in the panel, with subfolder navigation and history, F3 shows contents, F5 extracts the selected files and folders through the queue. Read-only mode, CRC verification, and the existing destination file is kept on error or cancellation.
- Photo viewer via F3: PNG/JPEG/BMP/GIF/WebP/ICO, fit to window, zoom and scrolling, respecting image orientation. Background reading and decoding, including from ZIP.
- "Tools → Open PowerShell / Terminal" (Ctrl+Shift+P) in the current folder; inside a ZIP, in the archive's folder.
- Select and deselect by mask (Ctrl+S / Ctrl+Shift+S), multiple masks separated by `;`, invert the visible selection (Ctrl+Shift+I).
- Undo move: after F6, cut-and-paste or a drag-and-drop move, the "Undo" button in the notification and Ctrl+Z return files to where they were (also after a cancelled or interrupted move — whatever had already been moved). Nothing is overwritten: if the original location is already taken, the file is skipped.
- English interface language. The language is chosen in "Settings → View and files → Interface language" and applied immediately, without a restart; on first launch the Windows language is used (Russian on a Russian system, English otherwise). Default favorites that have not been renamed get names in the selected language.
- A menu bar at the top of the window: "File", "Edit", "View", "Go", "Tools", "Help". It contains all commands with their keyboard shortcuts; items unavailable in the current folder are disabled, and enabled modes (two panels, hidden files, queue, case sensitivity, regex, sorting) are checked.
- "About" window: opened by clicking the Quokka logo or name, or from the "Help" menu. Program description, version and list of changes.
- Batch rename (Ctrl+M, context menu item, F2 with several items marked) with masks: `[N]`, `[E]` and their ranges (`[N2-5]`, `[N2,3]`, `[N-4-]`), counter `[C]` / `[C10+5:3]`, folder `[P]` / `[G]`, modification date and time `[Y-M-D]`, `[hms]`, size `[S]`, case `[U]` `[L]` `[F]` `[n]`. Plus find and replace (several replacements separated by `|`, or a regular expression with `$1`), case change, and a counter with start, step and number of digits. Live preview before renaming: conflicts (duplicate names, names already taken, invalid characters, reserved Windows names) are highlighted, and any file can be excluded with a checkbox. Swapping names (`a ↔ b`) is done safely, and on error everything is restored; Ctrl+Z or "Undo" in the notification reverts the rename. Mask history is saved.
- Tabs of both panels, the current tab and the active panel are saved and reopened on the next launch (can be turned off in "Settings → View and files"). A folder that no longer exists is replaced with the nearest existing one.
- Notification area (tray) icon: click shows the window, right-click opens a menu with "Open Quokka" / "Exit".
- "Minimize to tray on close" setting: the window's close button hides Quokka to the tray while background operations continue; exit with F10 or "Exit" in the icon menu.
- "Allow multiple Quokka windows" setting. Off by default: launching again brings up the already open window, and a folder passed on the command line opens there in a new tab.
- Ctrl+C / Ctrl+X / Ctrl+V to copy, cut and paste files into the current folder via the Windows clipboard, including exchange with Explorer and context menu items. Text fields keep the usual behaviour of these shortcuts.
- File-open animation on double-click, Enter or via the menu: row highlight, icon pulse and a brief "Opening…" indicator.
- Background size calculation for selected folders (Alt+Enter) or all folders in the panel via the context menu, with cancellation. Sizes appear in the column and are used for sorting; inaccessible items and links are marked as an incomplete result. Refreshing the list resets the calculation.
- Drag and drop of files with the mouse (by icon or name): between panels, onto a folder in the list, out to Explorer and other programs, and back into Quokka from them. Ctrl copies, Shift moves; with no key, files are moved within a drive and copied between drives, as in Explorer. An indicator next to the cursor shows what will happen; Esc cancels.
- File search (Alt+F7): masks with exclusions, regular expressions on names, text search inside files (case, whole words, regex, "does not contain", UTF-16 and Windows-1251), depth, size and date filters, hidden files, names inside ZIP. Results appear as the search runs; go to file, view, edit and "Files to panel". Mask and text history is saved.
- F4 opens the file in an external editor. The program is set in "Settings → View and files" (`%f` is the file path); Notepad by default. The built-in editor moved to Shift+F4.
- Undo delete to Recycle Bin: the "Undo" button in the notification and Ctrl+Z return files to where they were.
- Operation queue: copying, moving, deleting, recycling, archiving and extracting run one after another in the background without blocking the interface. Queue panel (Ctrl+J or the button in the top bar): progress, cancel, pause, reordering, retrying failed tasks.
- Extracting ZIP archives (Alt+F6), including several at once and with Cyrillic names in CP866.
- Exit confirmation while the queue has unfinished operations.

### Changed

- Redesigned column settings: a list with colored icons and sample values, prominent visibility checkboxes, a separate live preview area, a counter of enabled columns and compact hints.
- The code is split into layers following clean architecture principles: `quokka-domain`, `quokka-core` (use cases and ports), `quokka-infra` (adapters) and the UI. The core does not depend on egui, Windows or `std::fs` and is tested against an in-memory file system.
- Folders are read in the background: large, network and slow folders no longer freeze the window — "Loading…" is shown while the list is being read. Navigating to an unreachable network folder waits no longer than 0.4 s for a response.
- Open folders refresh automatically on changes on disk, including those made by other programs. After an operation only tabs with affected folders are refreshed; hidden tabs are refreshed when they are shown.
- File icons are cached by type: a folder with thousands of files of the same type loads the icon once.
- The drive list is read with a single OS call, and free space for sidebar tooltips is read in the background rather than every frame.
- Archiving (Alt+F5) and extracting (Alt+F6) put the result in the other panel by default, like copying (F5). If the other panel shows "This PC", the current folder is used. The path can be changed in the dialog.

### Fixed

- Columns are no longer hidden when the panel is narrowed: only the "Name" column width changes, and horizontal scrolling is available when space runs out. Long headers are truncated with `...`, and the full title is shown on hover.
- The system theme no longer resets button styles at startup: the configured padding is kept for both the light and dark OS themes.
- Quick single clicks on different files no longer open a file or trigger navigation in search results: a double-click requires two consecutive clicks on the same item.
- Padding of text buttons and menu items increased to 32 px horizontally and 10 px vertically and unified, including dialogs and search. Button rows in dialogs are taller. Menu item text no longer shifts on hover.
- Smooth column resizing without losing small mouse movements and without hidden columns appearing while dragging. The left and right panels keep their widths independently; old settings are carried over to both panels.

## [0.1.0] — 2026-11-20

First release.

### Added

- Dual-panel interface with tabs and switching to a single panel (F12).
- "This PC" view with the list of drives and free space.
- Sidebar with profile folders and drives.
- Breadcrumbs and manual path entry, back/forward history.
- Sorting by name, date, size and type; filtering by name.
- Marking files with the keyboard and mouse.
- Background copying, moving and deleting with progress and cancellation.
- Viewer (text / hex) and built-in UTF-8 editor.
- Creating files and folders, renaming.
- F1–F12 bar and keyboard shortcut help.
- Vector icons by file type and a quokka logo.
