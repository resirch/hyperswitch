# Hyperswitch

A minimal, fast Windows 11 Alt+Tab-style app switcher written in Rust. It runs
in the background, shows an overlay when you hold configurable modifiers and
press the cycle key, and focuses the selected window when those modifiers are
released.

## Build

Requirements: Rust (stable) with the `x86_64-pc-windows-msvc` target.

```
cargo build --release
```

The executable is produced at `target\release\hyperswitch.exe`.

Hyperswitch runs with administrator privileges (UAC prompt on launch). This is
required so it can switch away from elevated windows such as Task Manager.

## Run

```
target\release\hyperswitch.exe
```

Windows shows a UAC prompt the first time you launch it each session.

Only one instance runs at a time (enforced by a named mutex). A tray icon is
added; right-click it to toggle "Show title", "Current monitor only", and
"Run on startup" (changes are saved to the config file), or to "Reload config"
and "Exit". There is no console window.

Default usage:

- Hold your modifiers (default Ctrl+Alt+Win) and tap `C` to show the overlay and
  move forward through windows.
- Hold `Shift` while tapping `C` to move backward.
- `Tab`, arrow keys also cycle while the overlay is visible.
- `Esc` cancels without changing focus.
- Press any other key while the overlay is open (for example `Space` for another
  hyperkey shortcut) to cancel without switching; the key is passed through.
- Release the held modifiers to commit: the selected window is focused.

A quick tap-and-release behaves like classic Alt+Tab, jumping to the previously
focused window.

### Mouse (while the overlay is visible)

- Hover an icon to select it; release the held modifiers to switch to it.
- Left-click an icon to select and switch to that window immediately.
- Middle-click an icon to close that window (the overlay stays open so you can
  close several in a row).
- Click anywhere outside the icons, or right-click, to cancel without switching.

## Configuration

On first run, a config file is created at:

```
%APPDATA%\hyperswitch\config.toml
```

Options:

| Key                | Type   | Default | Description                                             |
| ------------------ | ------ | ------- | ------------------------------------------------------- |
| `hold_ctrl`        | bool   | true    | Require Ctrl as part of the activation modifiers.       |
| `hold_alt`         | bool   | true    | Require Alt as part of the activation modifiers.        |
| `hold_win`         | bool   | true    | Require Win as part of the activation modifiers.        |
| `hold_shift`       | bool   | false   | Require Shift as part of the activation modifiers.      |
| `cycle_key`        | string | "C"     | Key that advances the selection.                        |
| `reverse_modifier` | string | "Shift" | Modifier that reverses cycling direction.               |
| `opacity`          | int    | 235     | Background translucency, 0 - 255 (icons stay sharp).    |
| `icon_size`        | int    | 64      | Icon edge length in pixels (clamped to 16-256).         |
| `current_monitor_only` | bool | true | Only show windows on the monitor under the cursor.    |
| `show_title`       | bool   | true    | Draw the selected window's title below the icons.       |
| `run_on_startup`   | bool   | false   | Start Hyperswitch at sign-in (scheduled task).          |

`cycle_key` accepts a single letter or digit, or one of: `Tab`, `Space`,
`Backspace`, `Left`, `Right`, `Up`, `Down`, `Grave`.

The overlay is always centered on the monitor under the mouse cursor. With
`current_monitor_only = true`, the window list is additionally limited to
windows on that same monitor.

After editing the file, use the tray menu "Reload config" (no restart needed).

## Run on startup

### Without a UAC prompt at logon (recommended)

Use Task Scheduler so Hyperswitch starts elevated when you sign in, without a
UAC dialog each time. You only need admin approval once, when creating the task.

1. Open **Task Scheduler** (`Win+R`, type `taskschd.msc`, Enter).
2. Click **Create Task…** (not "Create Basic Task").
3. **General** tab:
   - Name: `Hyperswitch`
   - Select **Run only when user is logged on**
   - Check **Run with highest privileges**
4. **Triggers** tab → **New…** → **Begin the task:** At logon → **OK**
5. **Actions** tab → **New…** → **Action:** Start a program → **Program/script:**
   full path to `hyperswitch.exe` (for example
   `E:\Projects\hyperswitch\target\release\hyperswitch.exe`) → **OK**
6. **Conditions** tab (optional): uncheck **Start the task only if the computer
   is on AC power** if you use a laptop on battery.
7. Click **OK** to save. Approve the one-time UAC prompt if Windows asks while
   saving the task.

After that, Hyperswitch starts automatically at sign-in with administrator
privileges and no UAC prompt on each logon.

To remove it later: Task Scheduler → **Task Scheduler Library** → right-click
**Hyperswitch** → **Delete**.

### Tray / config option

You can also enable **Run on startup** from the tray menu, or set
`run_on_startup = true` in the config file. Hyperswitch creates the same kind
of logon scheduled task automatically. Disable the tray option (or set
`run_on_startup = false`) to remove that task.

Note: double-clicking `hyperswitch.exe` always shows a UAC prompt. Only the
scheduled-task-at-logon path avoids that on each sign-in.
