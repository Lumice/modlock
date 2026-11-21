# How Modlock changes the game

Your library stores mods and profiles. A profile selects the mods, file options and load order you want to use. Applying a profile installs that selection in the game. You can collect mods before choosing a Deadlock installation.

## Apply and Restore

With automatic Apply enabled, Modlock can apply supported changes while the game is closed. If Deadlock is running, close it before applying changes. Play can prepare a pending setup when you next start the game. If a selection or warning needs attention, review it first.

Modlock records a journal before changing the game. The journal records an operation so Modlock can check it after an interruption. Apply prepares the enabled mods in `game/citadel/deadlock_native/` and updates `game/citadel/gameinfo.gi` so the game can load them.

**Settings → Restore previous setup** restores the verified setup from before the previous Apply. If another tool changes the game files or the restore evidence is incomplete, Modlock can refuse the operation. Follow the message in the app before making further changes.

Crosshair and autoexec edits have separate save actions. Review the proposed change with Deadlock closed. Each successful save keeps a backup for **Undo last save**. Performance preset changes take effect through Apply or Play.

## Existing installations

Only one mod manager should manage a game setup at a time. If another tool changes the setup, Modlock will ask you to review it. To bring an existing mod library into Modlock, use **Settings → Import from the old Mod Manager…**.

On first use, Modlock checks both the current and historical library folders. If exactly one contains a library or its recovery records, it uses that folder. If both do, the startup message identifies them so you can choose the intended library.

## Where files live

| Data | Default location |
|---|---|
| New library database and profiles | `$XDG_DATA_HOME/modlock/`, normally `~/.local/share/modlock/` |
| Historical library | `$XDG_DATA_HOME/deadlock-native/`, normally `~/.local/share/deadlock-native/` |
| Bulk mod files, archives and download preparation | The library folder, or the storage location you choose in Settings |
| Display preferences | `$XDG_CONFIG_HOME/modlock/settings.conf`, normally `~/.config/modlock/settings.conf` |
| Recovery state | `$XDG_CONFIG_HOME/dl-mod-manager/state/`, normally `~/.config/dl-mod-manager/state/` |
| Applied mods | `<Deadlock>/game/citadel/deadlock_native/` |

**Move library storage** moves bulk files. The database and profiles stay in the library folder. A custom library root still uses the same recovery-state location. Modlock keeps historical staging names and cache paths so it can recognize existing installations.
